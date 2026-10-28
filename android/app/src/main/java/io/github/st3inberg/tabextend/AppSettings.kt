package io.github.st3inberg.tabextend

import android.content.Context

/** The tablet's own choices, kept across restarts. */
class AppSettings(context: Context) {
    private val prefs = context.getSharedPreferences("settings", Context.MODE_PRIVATE)

    var playSound: Boolean
        get() = prefs.getBoolean("play_sound", true)
        set(value) = prefs.edit().putBoolean("play_sound", value).apply()

    var showStats: Boolean
        get() = prefs.getBoolean("show_stats", false)
        set(value) = prefs.edit().putBoolean("show_stats", value).apply()
}
