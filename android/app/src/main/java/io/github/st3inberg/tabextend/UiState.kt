package io.github.st3inberg.tabextend

import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue

enum class Screen { Pairing, Home, Settings, Streaming }

/** Everything the screens show. Changed only on the main thread. */
class UiState {
    var screen by mutableStateOf(Screen.Pairing)
    /** The paired computer, as the user knows it. */
    var computerName by mutableStateOf<String?>(null)
    var status by mutableStateOf("")
    /** A connection attempt is running. */
    var busy by mutableStateOf(false)
    /** Why the last attempt stopped, if it gave up. */
    var error by mutableStateOf<String?>(null)
    /** The code to compare while pairing by code, and with which computer. */
    var code by mutableStateOf<String?>(null)
    var codeComputer by mutableStateOf("")
    var searching by mutableStateOf(false)
    var computers by mutableStateOf<List<FoundComputer>>(emptyList())
    var playSound by mutableStateOf(true)
    var showStats by mutableStateOf(false)
    var version by mutableStateOf("")
}

/** What the screens can ask for. */
interface Actions {
    fun scanQr()
    fun findComputers()
    fun pasteLink()
    fun pick(computer: FoundComputer)
    fun retry()
    fun openSettings()
    fun back()
    fun pairAnother()
    fun forget()
    fun setPlaySound(on: Boolean)
    fun setShowStats(on: Boolean)
    fun openSource()
}
