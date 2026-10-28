package io.github.st3inberg.tabextend

import android.os.Build
import androidx.activity.compose.BackHandler
import androidx.compose.foundation.Image
import androidx.compose.foundation.clickable
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.filled.Refresh
import androidx.compose.material.icons.filled.Settings
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ElevatedCard
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.ListItem
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.dynamicDarkColorScheme
import androidx.compose.material3.dynamicLightColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp

/** Material You colours where the tablet offers them, following light/dark mode. */
@Composable
fun TabExtendTheme(content: @Composable () -> Unit) {
    val dark = isSystemInDarkTheme()
    val context = LocalContext.current
    val colors = when {
        Build.VERSION.SDK_INT >= Build.VERSION_CODES.S ->
            if (dark) dynamicDarkColorScheme(context) else dynamicLightColorScheme(context)
        dark -> darkColorScheme()
        else -> lightColorScheme()
    }
    MaterialTheme(colorScheme = colors, content = content)
}

@Composable
fun App(state: UiState, actions: Actions) {
    when (state.screen) {
        Screen.Pairing -> PairingScreen(state, actions)
        Screen.Home -> HomeScreen(state, actions)
        Screen.Settings -> SettingsScreen(state, actions)
        Screen.Streaming -> {} // the video surface shows instead
    }
}

/** A centred column that stays readable on wide tablets. */
@Composable
private fun Page(padding: PaddingValues, content: @Composable () -> Unit) {
    Box(
        Modifier
            .fillMaxSize()
            .padding(padding)
            .verticalScroll(rememberScrollState()),
        contentAlignment = Alignment.TopCenter,
    ) {
        Column(
            Modifier
                .widthIn(max = 560.dp)
                .padding(24.dp),
            horizontalAlignment = Alignment.CenterHorizontally,
            verticalArrangement = Arrangement.spacedBy(16.dp),
        ) { content() }
    }
}

@Composable
private fun Illustration() {
    Image(
        painter = painterResource(R.drawable.ic_launcher),
        contentDescription = null,
        modifier = Modifier
            .size(96.dp),
    )
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun PairingScreen(state: UiState, actions: Actions) {
    if (state.computerName != null) BackHandler { actions.back() }
    Scaffold(topBar = {
        TopAppBar(
            title = { Text(stringResource(R.string.app_name)) },
            navigationIcon = {
                if (state.computerName != null) {
                    IconButton(onClick = actions::back) {
                        Icon(Icons.AutoMirrored.Filled.ArrowBack, stringResource(R.string.back))
                    }
                }
            },
        )
    }) { padding ->
        Page(padding) {
            val code = state.code
            if (code != null) {
                CodeCard(code, state.codeComputer)
                return@Page
            }
            Spacer(Modifier.height(24.dp))
            Illustration()
            Text(
                stringResource(R.string.pair_title),
                style = MaterialTheme.typography.headlineMedium,
                textAlign = TextAlign.Center,
            )
            Text(
                stringResource(R.string.pair_steps),
                style = MaterialTheme.typography.bodyLarge,
                textAlign = TextAlign.Center,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            state.error?.let {
                Text(it, color = MaterialTheme.colorScheme.error, textAlign = TextAlign.Center)
            }
            Spacer(Modifier.height(8.dp))
            Button(onClick = actions::scanQr, modifier = Modifier.fillMaxWidth()) {
                Text(stringResource(R.string.scan_qr))
            }
            OutlinedButton(onClick = actions::findComputers, modifier = Modifier.fillMaxWidth()) {
                Text(stringResource(R.string.find_computers))
            }
            TextButton(onClick = actions::pasteLink) {
                Text(stringResource(R.string.paste_link))
            }
            if (state.searching) ComputerList(state, actions)
        }
    }
}

@Composable
private fun ComputerList(state: UiState, actions: Actions) {
    Card(Modifier.fillMaxWidth()) {
        val pairable = state.computers.filter { it.pairing }
        if (pairable.isEmpty()) {
            Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
                Text(stringResource(R.string.searching), style = MaterialTheme.typography.bodyMedium)
                LinearProgressIndicator(Modifier.fillMaxWidth())
            }
        }
        pairable.forEachIndexed { i, computer ->
            if (i > 0) HorizontalDivider()
            ListItem(
                headlineContent = { Text(computer.name) },
                supportingContent = { Text(computer.address) },
                modifier = Modifier.clickableRow { actions.pick(computer) },
            )
        }
    }
}

@Composable
private fun CodeCard(code: String, computer: String) {
    Spacer(Modifier.height(24.dp))
    Text(
        stringResource(R.string.pairing_with, computer),
        style = MaterialTheme.typography.headlineSmall,
        textAlign = TextAlign.Center,
    )
    ElevatedCard(shape = RoundedCornerShape(24.dp)) {
        Text(
            code,
            style = MaterialTheme.typography.displayLarge,
            fontFamily = FontFamily.Monospace,
            modifier = Modifier.padding(horizontal = 40.dp, vertical = 24.dp),
        )
    }
    Text(
        stringResource(R.string.confirm_code, computer),
        style = MaterialTheme.typography.bodyLarge,
        textAlign = TextAlign.Center,
        color = MaterialTheme.colorScheme.onSurfaceVariant,
    )
    CircularProgressIndicator()
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun HomeScreen(state: UiState, actions: Actions) {
    Scaffold(topBar = {
        TopAppBar(
            title = { Text(stringResource(R.string.app_name)) },
            actions = {
                IconButton(onClick = actions::openSettings) {
                    Icon(Icons.Filled.Settings, stringResource(R.string.settings))
                }
            },
        )
    }) { padding ->
        Page(padding) {
            Spacer(Modifier.height(24.dp))
            Illustration()
            ElevatedCard(Modifier.fillMaxWidth()) {
                ListItem(
                    headlineContent = { Text(state.computerName ?: "") },
                    supportingContent = { Text(state.error ?: state.status) },
                    trailingContent = {
                        if (!state.busy) {
                            IconButton(onClick = actions::retry) {
                                Icon(Icons.Filled.Refresh, stringResource(R.string.try_again))
                            }
                        }
                    },
                )
                if (state.busy) LinearProgressIndicator(Modifier.fillMaxWidth())
            }
            Text(
                stringResource(R.string.home_hint),
                style = MaterialTheme.typography.bodyMedium,
                textAlign = TextAlign.Center,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun SettingsScreen(state: UiState, actions: Actions) {
    BackHandler { actions.back() }
    var confirmForget by remember { mutableStateOf(false) }
    Scaffold(topBar = {
        TopAppBar(
            title = { Text(stringResource(R.string.settings)) },
            navigationIcon = {
                IconButton(onClick = actions::back) {
                    Icon(Icons.AutoMirrored.Filled.ArrowBack, stringResource(R.string.back))
                }
            },
        )
    }) { padding ->
        Page(padding) {
            Card(Modifier.fillMaxWidth()) {
                ListItem(
                    headlineContent = { Text(stringResource(R.string.play_sound)) },
                    supportingContent = { Text(stringResource(R.string.play_sound_detail)) },
                    trailingContent = { Switch(state.playSound, actions::setPlaySound) },
                )
                HorizontalDivider()
                ListItem(
                    headlineContent = { Text(stringResource(R.string.show_stats)) },
                    supportingContent = { Text(stringResource(R.string.show_stats_detail)) },
                    trailingContent = { Switch(state.showStats, actions::setShowStats) },
                )
            }
            Card(Modifier.fillMaxWidth()) {
                ListItem(
                    headlineContent = { Text(stringResource(R.string.pair_another)) },
                    modifier = Modifier.clickableRow(actions::pairAnother),
                )
                HorizontalDivider()
                ListItem(
                    headlineContent = {
                        Text(
                            stringResource(R.string.forget_computer, state.computerName ?: ""),
                            color = MaterialTheme.colorScheme.error,
                        )
                    },
                    modifier = Modifier.clickableRow { confirmForget = true },
                )
            }
            Card(Modifier.fillMaxWidth()) {
                ListItem(
                    headlineContent = { Text(stringResource(R.string.version)) },
                    supportingContent = { Text(state.version) },
                )
                HorizontalDivider()
                ListItem(
                    headlineContent = { Text(stringResource(R.string.source_code)) },
                    supportingContent = { Text(stringResource(R.string.source_code_detail)) },
                    modifier = Modifier.clickableRow(actions::openSource),
                )
            }
        }
    }
    if (confirmForget) {
        AlertDialog(
            onDismissRequest = { confirmForget = false },
            title = { Text(stringResource(R.string.forget_title, state.computerName ?: "")) },
            text = { Text(stringResource(R.string.forget_detail)) },
            confirmButton = {
                TextButton(onClick = {
                    confirmForget = false
                    actions.forget()
                }) { Text(stringResource(R.string.forget)) }
            },
            dismissButton = {
                TextButton(onClick = { confirmForget = false }) { Text(stringResource(R.string.cancel)) }
            },
        )
    }
}

private fun Modifier.clickableRow(onClick: () -> Unit): Modifier = this.clickable(onClick = onClick)
