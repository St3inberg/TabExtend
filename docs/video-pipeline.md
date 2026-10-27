# Video pipeline

How a frame gets from GNOME to the tablet's screen, what each stage does to
keep latency low, and why. Measurements are from the reference hardware
(Fedora 44, GNOME 50, Intel TigerLake VA-API; Lenovo Idea Tab TB336FU with a
MediaTek `c2.mtk.avc.decoder`).

```
GNOME virtual monitor
  │  PipeWire, GPU buffers (DMA-BUF)              capture.rs
  ▼
GPU colour conversion (vapostproc)                pipeline.rs
  ▼
H.264 encoder, picked at runtime                  encoder.rs (core)
  ▼
SPS rewrite on keyframes                          h264.rs (core)
  ▼
frame gate: drop until keyframe if behind         stream.rs (core)
  │  TLS 1.3 over TCP, one message per frame      server.rs, protocol.rs
  ▼
tablet: hardware decoder, low-latency mode        VideoDecoder.kt
  ▼
output thread shows each frame as soon as ready
```

## Results

| Measure | Before this work | Now |
|---|---|---|
| Captured / sent while content moves | 12–16 fps, later 27–39 fps | 60 / 60 fps |
| Frames the decoder may hold back | up to 12 (declared buffer 4) | 1 declared, low-latency mode on |
| Decode time on the tablet | not measured | ~10 ms |
| Dropped frames | 0 | 0 |
| Wi-Fi round trip | 6–15 ms, spikes to ~110 ms | same |

The stats line on the host prints captured fps, sent fps, bitrate, round trip
and drops every 5 seconds; the tablet's corner shows rendered fps, bitrate
and average decode time. Captured equal to sent with no drops means the
network keeps up; a gap between them means it does not.

## 1. Capture

- **A virtual monitor from GNOME's portal**, sized by the tablet (step 5).
  GNOME treats it as a real display.
- **GPU buffers (DMA-BUF), no copies.** When the encoder chain is VA-API,
  the capture asks for `video/x-raw(memory:DMABuf)`, so GNOME hands over GPU
  memory and the VA-API converter imports it directly. Without it, GNOME
  reads every frame back into system memory and the converter uploads it
  again. If that format does not negotiate on a machine, the server
  rebuilds the pipeline once with copied frames (`stream_to` in server.rs).
- **Frames only when something changes.** GNOME produces a frame when the
  monitor's content changes, so a still screen costs nothing.
  `keepalive-time=500` resends the last frame every 500 ms, which is the
  2 fps seen when idle: a newly connected tablet always gets a picture.

## 2. Encode

**Choosing the encoder** (`select_encoder`, `encoder.rs`). Known encoders
are tried in priority order: VA-API low-power, VA-API, NVENC, Quick Sync,
x264, OpenH264. Each one is test-encoded with the real settings, not just
looked up, because an installed element can still fail (Fedora's
codec-stripped Intel driver) or ignore settings. The first encoder that
works and accepts every setting wins; one that works but rejects a setting
(Intel's low-power encoder has no constant-bitrate mode on this driver) is
only used if nothing better works. `--encoder` overrides the choice.

**Settings, all chosen for latency:**

| Setting | Why |
|---|---|
| No B-frames | A B-frame depends on a later frame, so the decoder must wait for it. |
| One reference frame (`ref-frames=1`) | Decoders size their picture buffer from the stream; one reference means almost nothing to hold. |
| Constant bitrate | Keeps frame sizes even, so Wi-Fi is never hit by bursts. |
| Fastest preset | Encode time is latency. |
| Keyframe every 5 s, plus on request | Keyframes are for recovery; with a reliable transport they are rarely needed. |
| Bitrate ≈ 0.1 bit per pixel per frame, 2–40 Mbit/s | Sharp text on mostly still desktops without filling the Wi-Fi link. |

Property names and units differ per encoder (OpenH264 takes bits per
second, the others kilobits), so each known encoder carries its own table.
Properties an installed version lacks are skipped and reported.

## 3. SPS rewrite (the decoder-buffering fix)

A decoder may hold decoded frames before showing them, in case a later
frame has to be shown first. The SPS (sequence parameter set, sent with
every keyframe) can rule that out in its VUI *bitstream restriction*:
`max_num_reorder_frames = 0` and a small `max_dec_frame_buffering`.

What the encoders on the reference laptop declare (checked with ffmpeg's
`trace_headers`):

| Encoder | Reference frames | Reordering | Decoder buffer |
|---|---|---|---|
| VA-API | 3 (1 with `ref-frames=1`) | 0 | 4 (2 with `ref-frames=1`) |
| Quick Sync | 2 | 0 | 2 |
| OpenH264 | 1 | 0 | 1 |
| NVENC | 3 | not declared | not declared |

So some encoders declare a larger buffer than they need, and NVENC declares
nothing, which lets the decoder assume the worst. The rewrite makes every
stream declare *no reordering, buffer = number of reference frames*. That is
the smallest value the standard allows, and true because B-frames are off.

**The algorithm** (`low_delay_access_unit` in `crates/core/src/h264.rs`):

1. Split the access unit at Annex B start codes (`00 00 01`) into NAL units;
   pick those of type 7 (SPS).
2. Remove emulation-prevention bytes (`00 00 03` → `00 00`) to get the raw
   bit payload.
3. Parse just far enough to find two positions: the
   `vui_parameters_present_flag` and, inside the VUI, the
   `bitstream_restriction_flag`. On the way, read `max_num_ref_frames`.
   This means walking every field before them: Exp-Golomb numbers, the
   High-profile chroma and bit-depth fields, scaling lists, picture order
   count variants, cropping, and in the VUI the aspect ratio, colour,
   timing and HRD blocks.
4. If the SPS already declares exactly what is wanted, change nothing.
5. Write a new payload: copy every bit before the restriction flag
   unchanged, then the restriction (keeping its existing leading fields, or
   the values the standard infers if it had none), with reordering 0 and
   buffer `max(1, max_num_ref_frames)`, then the stop bit and padding. An
   SPS without a VUI gets a minimal VUI holding only the restriction.
6. Re-insert emulation-prevention bytes and splice the new SPS back in; all
   other NAL units are kept byte for byte.

Anything that fails to parse is passed through unchanged: the rewrite can
make a stream better, never break it. Exp-Golomb lengths, scaling-list
counts and HRD counts are bounded, so malformed input cannot make it loop
or allocate without limit.

**How it was checked.** Unit tests run the rewrite on real SPS headers from
all four encoders above, plus a synthetic SPS without VUI, and check that
every field before the restriction is identical afterwards. The rewritten
streams were also read back by ffmpeg (`trace_headers`), which reports the
new values, and decoded by ffmpeg without errors. A pseudo-random input pass
checks that garbage never panics and that anything rewritten parses back.

This is the same idea game-streaming hosts such as Sunshine use.

## 4. Sending

- **One message per encoded frame** over TLS 1.3 on TCP, with `TCP_NODELAY`
  so small frames are not held back to be combined.
- **A queue of 3 frames.** Anything queued is latency, so the queue is kept
  short.
- **The frame gate** (`FrameGate` in `crates/core/src/stream.rs`). When the
  queue is full, sending an old frame only adds delay, so the frame is
  dropped. But an H.264 delta frame depends on the frames before it: after
  one drop, every following delta frame would decode as garbage. So after a
  drop the gate keeps dropping until the next keyframe, and asks the
  encoder for one, exactly once per outage. A new connection also waits for
  a keyframe, since a decoder cannot start on a delta frame.
- **TCP rather than UDP for now.** On the measured network (0 % loss) TCP's
  stall-on-loss does not show; the transport is isolated so QUIC can
  replace it if measurements on busier networks call for it (PRD,
  Transport).

## 5. The tablet

**Stream size from the decoder** (`StreamSize.fit`). The tablet asks the
host for the largest size its own decoder can decode at the target frame
rate, keeping the panel's aspect ratio: start at the panel size, and step
the width down 16 pixels at a time (height following the aspect ratio,
both rounded to even numbers) until the decoder's
`areSizeAndRateSupported` says yes. The TB336FU's decoder cannot do its
own panel's 2560×1600 (maximum 2560×1440, and about 1080p at 60 fps), so
it asks for 1808×1130. The decoded picture is scaled up to fill the panel.

**Low-latency decoding** (`VideoDecoder.kt`):

- `KEY_LOW_LATENCY` is always requested. The MediaTek decoder does not
  advertise the low-latency feature, yet honours the setting: its log shows
  `algo.low-latency` changing from 0 to 1. Decoders that lack it ignore it.
- The decoder's vendor parameters (Android 12+) are searched for any
  low-latency switch, which is turned on. This avoids hardcoding names per
  chip maker.
- Realtime priority (`KEY_PRIORITY = 0`).
- **A dedicated output thread** shows each frame the moment the decoder
  finishes it. Before, decoded frames were only collected when the next
  frame was fed in, so with uneven frame timing a finished frame could wait
  until the next one arrived, hundreds of milliseconds when the screen was
  nearly still. This was the largest single cause of the stutter.
- If the decoder has no free input buffer in time, the frame is dropped,
  the tablet asks the host for a keyframe, and delta frames are skipped
  until it arrives.

## Considered and not done

| Option | Why not (yet) |
|---|---|
| A playout buffer on the tablet to even out Wi-Fi bursts | Adds latency to every frame. Only worth it if measurements show bursts cause visible jitter after the fixes above. |
| UDP / QUIC | No loss measured on the reference network; TCP is simpler and native on Android. Revisit with measurements on busy networks. |
| HEVC | The tablet's HEVC decoder has the same size and rate limits as its H.264 decoder, and not every laptop can encode HEVC. |
| Typing a pairing code instead of comparing one | See [security.md](security.md): without a PAKE a machine in the middle could relay the typed code. |

## Remaining issues

- Occasional Wi-Fi round-trip spikes to ~110 ms, probably the tablet's
  Wi-Fi power saving. Not yet investigated.
- Input does not reach the virtual monitor yet ([#1](https://github.com/St3inberg/TabExtend/issues/1)),
  which also blocks a fully automated motion benchmark.
