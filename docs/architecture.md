# How TabExtend works

Diagrams of the whole program: the parts, how they talk, and the
algorithms that keep it fast and safe. For measurements and the reasoning
behind the video settings, see [video-pipeline.md](video-pipeline.md); for
the threat model, [security.md](security.md).

1. [The big picture](#1-the-big-picture)
2. [Code map](#2-code-map)
3. [A connection, start to finish](#3-a-connection-start-to-finish)
4. [Video: from GNOME to the tablet's screen](#4-video-from-gnome-to-the-tablets-screen)
5. [The frame gate (dropping frames safely)](#5-the-frame-gate)
6. [Choosing the stream size](#6-choosing-the-stream-size)
7. [Choosing the encoder](#7-choosing-the-encoder)
8. [Pairing by QR code](#8-pairing-by-qr-code)
9. [Pairing by code comparison](#9-pairing-by-code-comparison)
10. [Input: touch, pen, mouse, keyboard](#10-input)
11. [Sound](#11-sound)
12. [Finding computers on the network](#12-finding-computers-on-the-network)
13. [The tablet app's screens](#13-the-tablet-apps-screens)
14. [Reconnecting](#14-reconnecting)
15. [The desktop app's threads](#15-the-desktop-apps-threads)

## 1. The big picture

The computer runs the host; the tablet runs the app. GNOME creates a
virtual monitor for the tablet; the host captures it, encodes it as H.264
and sends it over Wi-Fi. Touches and keys travel back the other way.

```mermaid
flowchart LR
    subgraph PC["Computer: GNOME on Wayland"]
        Mutter["GNOME Shell / Mutter<br/>virtual monitor"]
        Portal["xdg-desktop-portal<br/>RemoteDesktop + ScreenCast"]
        PW["PipeWire<br/>screen + sound streams"]
        GST["GStreamer<br/>GPU colour convert + H.264 encode"]
        Host["TabExtend host<br/>server, pairing, input"]
        Avahi["Avahi<br/>mDNS announcement"]
        Mutter --> Portal --> PW --> GST --> Host
        Host -->|"libei input"| Mutter
        Host --> Avahi
    end
    subgraph Tab["Tablet: Android"]
        Session["StreamSession<br/>TLS client, protocol"]
        Dec["VideoDecoder<br/>MediaCodec, low latency"]
        Aud["AudioPlayer<br/>AudioTrack"]
        Inp["InputTranslator<br/>MotionEvent, KeyEvent"]
        UI["Compose screens<br/>pairing, home, settings"]
        Session --> Dec
        Session --> Aud
        Inp --> Session
        UI --- Session
    end
    Host <==>|"TLS 1.3 over TCP, port 47420<br/>video, sound, input, pings"| Session
    Avahi -.->|"_tabextend._tcp"| UI
```

## 2. Code map

```mermaid
flowchart TB
    subgraph core["crates/core: no I/O, fully unit-tested"]
        protocol["protocol.rs<br/>message format + size limits"]
        pairing["pairing.rs<br/>tokens, code, commitments, paired list"]
        stream["stream.rs<br/>stream size, bitrate, FrameGate"]
        h264["h264.rs<br/>SPS rewrite for low delay"]
        encoder["encoder.rs<br/>encoder list, tuning, selection"]
        input["input.rs<br/>InputMapper"]
    end
    subgraph host["crates/host: the program on the computer"]
        runner["runner.rs<br/>start host, mDNS, pairing window"]
        server["server.rs<br/>accept, handshake, streaming loop"]
        capture["capture.rs<br/>portal virtual monitor, test pattern"]
        pipeline["pipeline.rs / audio.rs<br/>GStreamer pipelines"]
        eis["input_eis.rs<br/>libei input"]
        mdns["mdns.rs<br/>Avahi over D-Bus"]
        identity["identity.rs<br/>keys + paired tablets on disk"]
        cli["main.rs<br/>tabextend CLI"]
    end
    subgraph app["crates/app: GTK 4 / libadwaita desktop app"]
        appmain["main.rs<br/>runtime, approver, actions"]
        window["window.rs<br/>status, stats, tablets"]
        dialogs["dialogs.rs<br/>pair, approve, preferences"]
    end
    subgraph android["android/: the tablet app (Kotlin)"]
        main["MainActivity<br/>screens + surface"]
        session["StreamSession<br/>connect, retry, stream"]
        tls["Tls<br/>pinned TLS client"]
        dec["VideoDecoder / AudioPlayer"]
        disc["Discovery<br/>NsdManager"]
        size["StreamSize<br/>decoder-limit search"]
    end
    cli --> runner
    appmain --> runner
    runner --> server
    server --> capture & pipeline & eis
    runner --> mdns & identity
    server --> core
    main --> session --> tls
    session --> dec
    main --> disc & size
```

## 3. A connection, start to finish

```mermaid
sequenceDiagram
    autonumber
    participant T as Tablet
    participant H as Host
    participant G as GNOME (portal)
    T->>H: TCP connect, port 47420
    T->>H: TLS 1.3 handshake, both sides present certificates
    Note over T,H: Tablet checks the host's fingerprint (pinned at pairing).<br/>Host reads the tablet's fingerprint.
    T->>H: Hello {version, pairing, device name, display size + refresh}
    alt wrong version / not paired / another tablet streaming / bad size
        H-->>T: Reject {reason, detail}
    end
    H->>G: create virtual monitor (stream size), start screencast + libei
    G-->>H: PipeWire stream ready
    H->>T: Welcome {codec H.264, width, height}
    loop while connected
        H->>T: Video {pts, keyframe, data}  (each frame)
        H->>T: Audio {pts, PCM}  (if sound is on)
        H->>T: Ping (every second)
        T->>H: Pong  → round-trip time for the stats
        T->>H: Input {touch / pointer / button / scroll / key}
        T->>H: RequestKeyframe (decoder (re)started)
    end
    T--xH: connection closes
    H->>G: stop screencast, monitor disappears
```

## 4. Video: from GNOME to the tablet's screen

Every stage avoids buffering: a frame that waits is a frame that arrives
late.

```mermaid
flowchart TD
    A["GNOME virtual monitor"] -->|"PipeWire, DMA-BUF<br/>(frame stays on the GPU)"| B["vapostproc<br/>RGB → NV12 on the GPU"]
    B --> C["H.264 encoder<br/>chosen at runtime, 1 reference frame,<br/>no B-frames, keyframe every 5 s"]
    C --> D["h264parse"]
    D --> E["SPS rewrite on keyframes<br/>tell the decoder: show every frame at once"]
    E --> F{"FrameGate<br/>room in the 3-frame send queue?"}
    F -->|yes| G["TLS write, one message per frame<br/>TCP_NODELAY"]
    F -->|no| X["drop, wait for keyframe"]
    G -->|Wi-Fi| H["StreamSession reads the message"]
    H --> I["MediaCodec decoder<br/>KEY_LOW_LATENCY + vendor low-latency keys"]
    I --> J["output thread releases each frame<br/>to the Surface the moment it is decoded"]
    B -.->|"GPU buffers fail before the first frame"| K["restart once with copied frames"]
```

## 5. The frame gate

When Wi-Fi cannot keep up, sending old frames only adds delay, so frames
are dropped. But an H.264 delta frame only makes sense on top of the
frames before it, so after one drop every delta frame is useless until the
next keyframe. The gate (`crates/core/src/stream.rs`) handles this:

```mermaid
stateDiagram-v2
    [*] --> AwaitingKeyframe: new stream (decoders start on a keyframe)
    AwaitingKeyframe --> Flowing: keyframe and queue has room → send
    AwaitingKeyframe --> AwaitingKeyframe: delta frame → drop, ask the encoder for a keyframe once
    Flowing --> Flowing: queue has room → send
    Flowing --> AwaitingKeyframe: queue full → drop + request keyframe
    Flowing --> AwaitingKeyframe: tablet sends RequestKeyframe
```

## 6. Choosing the stream size

The tablet asks for the largest picture its decoder can handle at its
refresh rate; the host may shrink it further (Preferences → Resolution).

```mermaid
flowchart TD
    P["Tablet panel, e.g. 2560×1600 at 90 Hz"] --> R["frame rate = min(refresh, 60)"]
    R --> S["width = panel width"]
    S --> Q{"decoder supports<br/>width × height at that rate?<br/>(also tried rotated)"}
    Q -->|no| M["width −= 16, height keeps the aspect ratio,<br/>both rounded to even"] --> Q
    Q -->|yes| H["Hello: e.g. 1808×1130 @ 60"]
    H --> HS{"host has a max width<br/>smaller than that?"}
    HS -->|yes| SC["scale down, keep aspect, round to even"]
    HS -->|no| K["keep"]
    SC & K --> BR["bitrate = width × height × fps × 0.1 bit<br/>clamped to 2–40 Mbit/s"]
    BR --> V["GNOME creates the monitor at this size;<br/>the tablet scales the picture to fill the panel"]
```

## 7. Choosing the encoder

Nothing is assumed about the hardware: each candidate is tried with a real
test encode, in this order, and the first that works is used. A choice
saved in Preferences goes first.

```mermaid
flowchart LR
    pref["saved choice<br/>(if any)"] --> va["vah264enc<br/>VA-API: Intel / AMD"] --> nv["nvh264enc<br/>NVENC: NVIDIA"] --> qsv["qsvh264enc<br/>Quick Sync: Intel"] --> x264["x264enc<br/>software"] --> oh["openh264enc<br/>software"]
    va & nv & qsv & x264 & oh -.->|"probe: build pipeline,<br/>encode test frames"| ok(["first that works"])
```

`tabextend doctor` (or System Check in the app) prints the result of every
probe.

## 8. Pairing by QR code

The QR code carries the host's address, its certificate fingerprint, and a
one-time token. Scanning it gives the tablet everything it needs to trust
the host; the token proves to the host that the tablet saw the screen.

```mermaid
sequenceDiagram
    autonumber
    participant U as You
    participant H as Host (desktop app)
    participant T as Tablet
    U->>H: Pair a Tablet
    H->>H: open pairing window: random token,<br/>valid 2 minutes, one use, 5 wrong tries max
    H-->>U: QR code: tabextend://pair?host=…&port=…&fp=…&token=…
    U->>T: Scan QR code
    T->>H: TLS, checking the host certificate against fp
    T->>H: Hello {pairing: Token}
    H->>H: token right and window open?<br/>→ remember the tablet's fingerprint, close window
    H->>T: Welcome, streaming starts
    T->>T: remember host address + fingerprint
```

## 9. Pairing by code comparison

Without a camera, the tablet finds the computer on the network and both
screens show a 6-digit code. Neither side knows the other's key yet, so
the code is what proves no one is in between. A commitment stops an
attacker from trying many keys until the codes match: the tablet locks in
its secret before it sees the host's.

```mermaid
sequenceDiagram
    autonumber
    participant T as Tablet
    participant H as Host
    participant U as You
    T->>H: TLS (fingerprints not checked yet: both just recorded)
    T->>T: pick secret nonce Nt
    T->>H: Hello {pairing: Confirm {commitment = SHA-256("tabextend commitment v1" ‖ Nt)}}
    H->>H: pairing window open? (2 min, 5 refusals max)
    H->>T: PairChallenge {Nh}  (host's random nonce)
    T->>H: PairReveal {Nt}
    H->>H: check SHA-256(Nt) = commitment
    Note over T,H: code = SHA-256("tabextend confirmation v1" ‖ host fp ‖ tablet fp ‖ Nh ‖ Nt)<br/>first 4 bytes mod 1 000 000, shown as "123 456"
    H-->>U: Allow "Tablet" with code 778 062?
    T-->>U: shows 778 062
    U->>H: Allow (codes match)
    H->>H: remember the tablet
    H->>T: Welcome, streaming starts
    T->>T: remember the host fingerprint
```

## 10. Input

```mermaid
flowchart LR
    subgraph Tablet
        ME["MotionEvent<br/>fingers, pen, mouse"] --> IT["InputTranslator<br/>scale to stream pixels,<br/>track up to 10 finger slots"]
        KE["KeyEvent<br/>hardware keyboard"] --> IT
    end
    IT -->|"Input: Touch / PointerMove /<br/>Button / Scroll / Key"| IM
    subgraph Host
        IM["InputMapper<br/>clip to the monitor, drop NaN,<br/>release everything on disconnect"] --> EIS["libei (ConnectToEIS)<br/>regions matched to the virtual monitor"]
        EIS --> GN["GNOME applies it<br/>on the tablet's monitor"]
        EIS -.->|"desktop refuses touch"| EM["fingers drive the pointer instead<br/>(tap = click, drag = drag)"]
    end
```

## 11. Sound

```mermaid
flowchart LR
    App["any app on the computer"] --> Sink["PipeWire sink<br/>'TabExtend: tablet name'<br/>(appears while connected)"]
    Sink --> Cap["pipewiresrc → 48 kHz stereo S16"] --> Q["16-chunk queue<br/>(full → drop: stale sound is worse than a gap)"]
    Q -->|"Audio messages"| AT["AudioTrack, low latency,<br/>~60 ms buffer"] --> Spk["tablet speakers / headphones"]
```

You pick the tablet in the computer's sound settings like any speaker; the
"Play computer sound" switch on the tablet turns playback off.

## 12. Finding computers on the network

```mermaid
flowchart LR
    H["Host"] -->|"always: _tabextend._tcp,<br/>short id only"| N(("Local network<br/>mDNS"))
    H -->|"while the pairing window is open:<br/>computer name + 'pairing' flag"| N
    N --> D["Tablet: NsdManager discovery"] --> L["'On this network' list<br/>shows only computers in pairing mode"]
```

The computer's name is only announced while you are pairing, so the
network does not learn it the rest of the time.

## 13. The tablet app's screens

```mermaid
stateDiagram-v2
    [*] --> Splash
    Splash --> Pairing: no computer paired
    Splash --> Home: computer paired
    Pairing --> Pairing: searching the network (automatic)
    Pairing --> CodeCheck: picked a computer
    CodeCheck --> Streaming: allowed on the computer
    Pairing --> Streaming: QR code / link accepted
    Home --> Streaming: computer answered
    Home --> Home: Waiting for computer (retrying quietly)
    Streaming --> Home: connection lost
    Home --> Settings
    Settings --> Home: back
    Settings --> Pairing: pair another / forget
```

## 14. Reconnecting

```mermaid
flowchart TD
    C["connect"] --> R{"result"}
    R -->|"streamed, then lost"| B0["wait 0.5 s"] --> C
    R -->|"nothing answers"| W["'Waiting for computer'<br/>no spinner"] --> B["wait: 0.5 s, 1 s, 2 s, 4 s, then every 5 s"] --> C
    R -->|"other error"| E["show the error"] --> B
    R -->|"pairing by code failed"| G["give up, show why"]
```

So the tablet connects by itself within about five seconds of TabExtend
starting on the computer.

## 15. The desktop app's threads

```mermaid
flowchart LR
    subgraph GTK["GTK main thread (never blocks)"]
        W["Window, dialogs"]
    end
    subgraph RT["Tokio runtime (background)"]
        S["Server: accept, stream loops"]
        A["DialogApprover"]
    end
    subgraph GS["GStreamer threads"]
        P["capture + encode"]
        AU["sound pipeline<br/>(own thread: starting it can block)"]
    end
    S -->|"ServerEvent<br/>(async channel)"| W
    A -->|"ApprovalRequest"| W
    W -->|"answer (oneshot)"| A
    P -->|"encoded frames"| S
    AU -->|"PCM chunks"| S
```
