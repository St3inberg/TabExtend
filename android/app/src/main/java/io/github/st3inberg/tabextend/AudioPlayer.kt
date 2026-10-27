package io.github.st3inberg.tabextend

import android.media.AudioAttributes
import android.media.AudioFormat
import android.media.AudioTrack

/**
 * Plays the laptop's sound as it arrives.
 *
 * Low-latency output with a small buffer (about 60 ms): enough to ride out
 * Wi-Fi jitter, little enough not to drift behind the picture. Writes never
 * block; when the buffer is full the excess is dropped rather than letting
 * delay build up.
 */
class AudioPlayer {
    private val track: AudioTrack

    init {
        val channels = AudioFormat.CHANNEL_OUT_STEREO
        val encoding = AudioFormat.ENCODING_PCM_16BIT
        val minimum = AudioTrack.getMinBufferSize(Protocol.AUDIO_RATE, channels, encoding)
        track = AudioTrack.Builder()
            .setAudioAttributes(
                AudioAttributes.Builder()
                    .setUsage(AudioAttributes.USAGE_MEDIA)
                    .setContentType(AudioAttributes.CONTENT_TYPE_MUSIC)
                    .build(),
            )
            .setAudioFormat(
                AudioFormat.Builder()
                    .setSampleRate(Protocol.AUDIO_RATE)
                    .setChannelMask(channels)
                    .setEncoding(encoding)
                    .build(),
            )
            .setBufferSizeInBytes(maxOf(minimum, BUFFER_BYTES))
            .setPerformanceMode(AudioTrack.PERFORMANCE_MODE_LOW_LATENCY)
            .setTransferMode(AudioTrack.MODE_STREAM)
            .build()
        track.play()
    }

    fun write(pcm: ByteArray) {
        track.write(pcm, 0, pcm.size - pcm.size % FRAME_BYTES, AudioTrack.WRITE_NON_BLOCKING)
    }

    fun release() {
        runCatching { track.stop() }
        track.release()
    }

    private companion object {
        /** One stereo 16-bit sample frame. */
        const val FRAME_BYTES = 4
        /** About 60 ms at 48 kHz stereo 16-bit. */
        const val BUFFER_BYTES = Protocol.AUDIO_RATE * FRAME_BYTES * 60 / 1000
    }
}
