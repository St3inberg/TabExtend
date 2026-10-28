package io.github.st3inberg.tabextend

import android.os.Build
import androidx.activity.compose.BackHandler
import androidx.annotation.DrawableRes
import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.core.LinearEasing
import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.animation.expandVertically
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.shrinkVertically
import androidx.compose.animation.togetherWith
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.filled.Add
import androidx.compose.material.icons.filled.Delete
import androidx.compose.material.icons.filled.Info
import androidx.compose.material.icons.filled.Refresh
import androidx.compose.material.icons.filled.Search
import androidx.compose.material.icons.filled.Settings
import androidx.compose.material.icons.filled.Warning
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FilledTonalButton
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.ListItem
import androidx.compose.material3.ListItemDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Surface
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.TopAppBarDefaults
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
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp

private val BrandLight = lightColorScheme(
    primary = Color(0xFF2B5C8F),
    primaryContainer = Color(0xFFD3E4FF),
    onPrimaryContainer = Color(0xFF001C38),
    tertiary = Color(0xFF3F6B8C),
    tertiaryContainer = Color(0xFFC6E7FF),
)

private val BrandDark = darkColorScheme(
    primary = Color(0xFFA2C9FF),
    primaryContainer = Color(0xFF0E4475),
    onPrimaryContainer = Color(0xFFD3E4FF),
    tertiary = Color(0xFF8BCEFF),
    tertiaryContainer = Color(0xFF1E4D6E),
)

/** Material You colours where the tablet offers them, following light/dark mode. */
@Composable
fun TabExtendTheme(content: @Composable () -> Unit) {
    val dark = isSystemInDarkTheme()
    val context = LocalContext.current
    val colors = when {
        Build.VERSION.SDK_INT >= Build.VERSION_CODES.S ->
            if (dark) dynamicDarkColorScheme(context) else dynamicLightColorScheme(context)
        dark -> BrandDark
        else -> BrandLight
    }
    MaterialTheme(colorScheme = colors, content = content)
}

@Composable
fun App(state: UiState, actions: Actions) {
    AnimatedContent(
        targetState = state.screen,
        transitionSpec = { fadeIn(tween(220)) togetherWith fadeOut(tween(150)) },
        label = "screen",
    ) { screen ->
        when (screen) {
            Screen.Pairing -> PairingScreen(state, actions)
            Screen.Home -> HomeScreen(state, actions)
            Screen.Settings -> SettingsScreen(state, actions)
            Screen.Streaming -> Box(Modifier.fillMaxSize()) // the video surface shows instead
        }
    }
}

/**
 * The illustration: a laptop and a tablet. While connecting, a window
 * travels from one to the other; after a failure the tablet turns red.
 */
@Composable
private fun Hero(active: Boolean, failed: Boolean, modifier: Modifier = Modifier) {
    val colors = MaterialTheme.colorScheme
    val travel by rememberInfiniteTransition(label = "hero").animateFloat(
        initialValue = 0f,
        targetValue = 1f,
        animationSpec = infiniteRepeatable(tween(1600, easing = LinearEasing), RepeatMode.Restart),
        label = "travel",
    )
    val progress = if (active) travel else 1f
    val tabletFill = if (failed) colors.errorContainer else colors.tertiaryContainer
    val tabletEdge = if (failed) colors.error else colors.tertiary
    Canvas(modifier.widthIn(max = 440.dp).aspectRatio(1.9f)) {
        val w = size.width
        val h = size.height
        val corner = CornerRadius(w * 0.03f)
        // Laptop: screen with a desktop's worth of windows, then the base.
        drawRoundRect(colors.primaryContainer, Offset(w * 0.04f, h * 0.1f), Size(w * 0.56f, h * 0.66f), corner)
        drawRoundRect(colors.primary, Offset(w * 0.04f, h * 0.1f), Size(w * 0.56f, h * 0.66f), corner, Stroke(w * 0.012f))
        drawRoundRect(
            colors.onPrimaryContainer.copy(alpha = 0.18f),
            Offset(w * 0.09f, h * 0.2f), Size(w * 0.22f, h * 0.24f), CornerRadius(w * 0.012f),
        )
        drawRoundRect(
            colors.onPrimaryContainer.copy(alpha = 0.12f),
            Offset(w * 0.09f, h * 0.5f), Size(w * 0.3f, h * 0.16f), CornerRadius(w * 0.012f),
        )
        drawRoundRect(colors.primary, Offset(0f, h * 0.8f), Size(w * 0.64f, h * 0.07f), CornerRadius(w * 0.02f))
        // Tablet.
        val tabletLeft = w * 0.7f
        drawRoundRect(tabletFill, Offset(tabletLeft, h * 0.06f), Size(w * 0.27f, h * 0.86f), corner)
        drawRoundRect(tabletEdge, Offset(tabletLeft, h * 0.06f), Size(w * 0.27f, h * 0.86f), corner, Stroke(w * 0.012f))
        // The window that moves onto the tablet.
        if (!failed) {
            val startX = w * 0.36f
            val endX = tabletLeft + w * 0.035f
            val x = startX + (endX - startX) * easeInOut(progress)
            val fade = if (active) 1f - kotlin.math.max(0f, progress - 0.85f) / 0.15f else 1f
            drawRoundRect(
                colors.primary.copy(alpha = 0.85f * fade),
                Offset(x, h * 0.3f), Size(w * 0.2f, h * 0.3f), CornerRadius(w * 0.015f),
            )
        }
        // Signal dots between the two while connecting.
        if (active) {
            for (i in 0..2) {
                val phase = (travel * 3f - i).coerceIn(0f, 1f)
                drawCircle(
                    colors.tertiary.copy(alpha = 0.25f + 0.75f * phase),
                    radius = w * 0.008f,
                    center = Offset(w * (0.625f + i * 0.025f), h * 0.45f),
                )
            }
        }
    }
}

private fun easeInOut(t: Float) = if (t < 0.5f) 2 * t * t else 1 - (-2 * t + 2).let { it * it } / 2

/**
 * A page: on a wide landscape screen the illustration and title sit on the
 * left and [content] on the right; otherwise everything is one column.
 */
@Composable
private fun Page(
    padding: PaddingValues,
    hero: @Composable ColumnScope.() -> Unit,
    content: @Composable ColumnScope.() -> Unit,
) {
    BoxWithConstraints(
        Modifier
            .fillMaxSize()
            .padding(padding),
    ) {
        if (maxWidth >= 840.dp && maxWidth > maxHeight) {
            Row(
                Modifier
                    .fillMaxSize()
                    .padding(horizontal = 48.dp),
                horizontalArrangement = Arrangement.spacedBy(64.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Column(
                    Modifier.weight(1f).widthIn(max = 520.dp),
                    verticalArrangement = Arrangement.spacedBy(20.dp),
                    content = hero,
                )
                Column(
                    Modifier
                        .weight(1f)
                        .widthIn(max = 520.dp)
                        .verticalScroll(rememberScrollState())
                        .padding(vertical = 24.dp),
                    verticalArrangement = Arrangement.spacedBy(12.dp),
                    content = content,
                )
            }
        } else {
            Column(
                Modifier
                    .fillMaxSize()
                    .verticalScroll(rememberScrollState())
                    .padding(24.dp),
                horizontalAlignment = Alignment.CenterHorizontally,
            ) {
                Column(
                    Modifier.widthIn(max = 560.dp),
                    verticalArrangement = Arrangement.spacedBy(12.dp),
                ) {
                    hero()
                    Spacer(Modifier.height(8.dp))
                    content()
                }
            }
        }
    }
}

@Composable
private fun Headline(title: String, body: String) {
    Text(title, style = MaterialTheme.typography.headlineLarge, fontWeight = FontWeight.SemiBold)
    Text(body, style = MaterialTheme.typography.bodyLarge, color = MaterialTheme.colorScheme.onSurfaceVariant)
}

/** A large tappable choice with an icon, as on the pairing screen. */
@Composable
private fun OptionCard(
    icon: Painter,
    title: String,
    detail: String,
    prominent: Boolean = false,
    onClick: () -> Unit,
) {
    val colors = MaterialTheme.colorScheme
    Card(
        onClick = onClick,
        modifier = Modifier.fillMaxWidth(),
        shape = RoundedCornerShape(20.dp),
        colors = CardDefaults.cardColors(
            containerColor = if (prominent) colors.primaryContainer else colors.surfaceContainerHigh,
            contentColor = if (prominent) colors.onPrimaryContainer else colors.onSurface,
        ),
    ) {
        Row(
            Modifier.padding(20.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(16.dp),
        ) {
            Box(
                Modifier
                    .size(48.dp)
                    .clip(CircleShape)
                    .background(if (prominent) colors.primary else colors.secondaryContainer),
                contentAlignment = Alignment.Center,
            ) {
                icon.Draw(tint = if (prominent) colors.onPrimary else colors.onSecondaryContainer)
            }
            Column(Modifier.weight(1f)) {
                Text(title, style = MaterialTheme.typography.titleMedium)
                Text(
                    detail,
                    style = MaterialTheme.typography.bodyMedium,
                    color = if (prominent) colors.onPrimaryContainer.copy(alpha = 0.8f) else colors.onSurfaceVariant,
                )
            }
        }
    }
}

/** An icon from the core set or from this app's drawables. */
private sealed interface Painter {
    @Composable
    fun Draw(tint: Color)
}

private class VectorIcon(val vector: ImageVector) : Painter {
    @Composable
    override fun Draw(tint: Color) = Icon(vector, null, tint = tint)
}

private class DrawableIcon(@param:DrawableRes val id: Int) : Painter {
    @Composable
    override fun Draw(tint: Color) = Icon(painterResource(id), null, tint = tint)
}

@Composable
private fun ErrorBanner(text: String) {
    Surface(
        color = MaterialTheme.colorScheme.errorContainer,
        contentColor = MaterialTheme.colorScheme.onErrorContainer,
        shape = RoundedCornerShape(16.dp),
        modifier = Modifier.fillMaxWidth(),
    ) {
        Row(Modifier.padding(16.dp), horizontalArrangement = Arrangement.spacedBy(12.dp)) {
            Icon(Icons.Filled.Warning, null)
            Text(text, style = MaterialTheme.typography.bodyMedium)
        }
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun PlainTopBar(title: String, onBack: (() -> Unit)?, actions: @Composable () -> Unit = {}) {
    TopAppBar(
        title = { Text(title) },
        navigationIcon = {
            if (onBack != null) {
                IconButton(onClick = onBack) {
                    Icon(Icons.AutoMirrored.Filled.ArrowBack, stringResource(R.string.back))
                }
            }
        },
        actions = { actions() },
        colors = TopAppBarDefaults.topAppBarColors(containerColor = Color.Transparent),
    )
}

@Composable
private fun PairingScreen(state: UiState, actions: Actions) {
    val canGoBack = state.computerName != null
    if (canGoBack) BackHandler { actions.back() }
    Scaffold(topBar = {
        PlainTopBar(stringResource(R.string.app_name), if (canGoBack) actions::back else null)
    }) { padding ->
        val code = state.code
        if (code != null) {
            Page(padding, hero = { Hero(active = true, failed = false) }) {
                CodeCard(code, state.codeComputer)
            }
            return@Scaffold
        }
        Page(
            padding,
            hero = {
                Hero(active = false, failed = false)
                Headline(stringResource(R.string.pair_title), stringResource(R.string.pair_steps))
            },
        ) {
            state.error?.let { ErrorBanner(it) }
            // Searching starts by itself, so the computer usually appears here first.
            AnimatedVisibility(state.searching, enter = expandVertically() + fadeIn(), exit = shrinkVertically() + fadeOut()) {
                ComputerList(state, actions)
            }
            OptionCard(
                DrawableIcon(R.drawable.ic_qr),
                stringResource(R.string.scan_qr),
                stringResource(R.string.scan_qr_detail),
                prominent = true,
                onClick = actions::scanQr,
            )
            if (!state.searching) {
                OptionCard(
                    VectorIcon(Icons.Filled.Search),
                    stringResource(R.string.find_computers),
                    stringResource(R.string.find_computers_detail),
                    onClick = actions::findComputers,
                )
            }
            OptionCard(
                DrawableIcon(R.drawable.ic_paste),
                stringResource(R.string.paste_link),
                stringResource(R.string.paste_link_detail),
                onClick = actions::pasteLink,
            )
        }
    }
}

@Composable
private fun ComputerList(state: UiState, actions: Actions) {
    Card(
        Modifier.fillMaxWidth(),
        shape = RoundedCornerShape(20.dp),
        colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surfaceContainerLow),
    ) {
        Text(
            stringResource(R.string.on_network),
            style = MaterialTheme.typography.labelLarge,
            color = MaterialTheme.colorScheme.primary,
            modifier = Modifier.padding(start = 20.dp, top = 16.dp, end = 20.dp),
        )
        val pairable = state.computers.filter { it.pairing }
        if (pairable.isEmpty()) {
            Column(Modifier.padding(20.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
                Text(stringResource(R.string.searching), style = MaterialTheme.typography.bodyMedium)
                LinearProgressIndicator(Modifier.fillMaxWidth())
            }
        }
        pairable.forEach { computer ->
            ListItem(
                headlineContent = { Text(computer.name) },
                supportingContent = { Text(computer.address) },
                leadingContent = { Icon(painterResource(R.drawable.ic_computer), null) },
                colors = ListItemDefaults.colors(containerColor = Color.Transparent),
                modifier = Modifier.clickable { actions.pick(computer) },
            )
        }
        Spacer(Modifier.height(8.dp))
    }
}

@Composable
private fun CodeCard(code: String, computer: String) {
    Text(
        stringResource(R.string.pairing_with, computer),
        style = MaterialTheme.typography.headlineMedium,
        fontWeight = FontWeight.SemiBold,
    )
    Text(
        stringResource(R.string.confirm_code, computer),
        style = MaterialTheme.typography.bodyLarge,
        color = MaterialTheme.colorScheme.onSurfaceVariant,
    )
    Spacer(Modifier.height(8.dp))
    Row(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
        code.forEach { c ->
            if (c == ' ') {
                Spacer(Modifier.width(12.dp))
            } else {
                Surface(
                    color = MaterialTheme.colorScheme.primaryContainer,
                    contentColor = MaterialTheme.colorScheme.onPrimaryContainer,
                    shape = RoundedCornerShape(14.dp),
                ) {
                    Text(
                        c.toString(),
                        style = MaterialTheme.typography.displayMedium,
                        fontFamily = FontFamily.Monospace,
                        modifier = Modifier.padding(horizontal = 14.dp, vertical = 10.dp),
                    )
                }
            }
        }
    }
    Spacer(Modifier.height(8.dp))
    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
        CircularProgressIndicator(Modifier.size(20.dp), strokeWidth = 2.dp)
        Text(stringResource(R.string.waiting_for_allow), style = MaterialTheme.typography.bodyMedium)
    }
}

@Composable
private fun StatusChip(text: String, color: Color, onColor: Color) {
    Surface(color = color, contentColor = onColor, shape = RoundedCornerShape(50)) {
        Row(
            Modifier.padding(horizontal = 12.dp, vertical = 6.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            Box(
                Modifier
                    .size(8.dp)
                    .clip(CircleShape)
                    .background(onColor),
            )
            Text(text, style = MaterialTheme.typography.labelLarge)
        }
    }
}

@Composable
private fun HomeScreen(state: UiState, actions: Actions) {
    val colors = MaterialTheme.colorScheme
    val failed = state.error != null && !state.busy
    Scaffold(topBar = {
        PlainTopBar(stringResource(R.string.app_name), onBack = null) {
            IconButton(onClick = actions::openSettings) {
                Icon(Icons.Filled.Settings, stringResource(R.string.settings))
            }
        }
    }) { padding ->
        Page(
            padding,
            hero = {
                Hero(active = state.busy, failed = failed)
                when {
                    state.busy -> StatusChip(stringResource(R.string.status_connecting), colors.tertiaryContainer, colors.onTertiaryContainer)
                    failed -> StatusChip(stringResource(R.string.status_offline), colors.errorContainer, colors.onErrorContainer)
                    else -> StatusChip(stringResource(R.string.status_waiting), colors.secondaryContainer, colors.onSecondaryContainer)
                }
                Headline(state.computerName ?: "", state.error ?: state.status)
                if (!state.busy && !state.waiting) {
                    FilledTonalButton(onClick = actions::retry) {
                        Icon(Icons.Filled.Refresh, null, Modifier.size(18.dp))
                        Spacer(Modifier.width(8.dp))
                        Text(stringResource(R.string.try_again))
                    }
                }
            },
        ) {
            Card(
                Modifier.fillMaxWidth(),
                shape = RoundedCornerShape(20.dp),
                colors = CardDefaults.cardColors(containerColor = colors.surfaceContainerLow),
            ) {
                Text(
                    stringResource(R.string.tips),
                    style = MaterialTheme.typography.labelLarge,
                    color = colors.primary,
                    modifier = Modifier.padding(start = 20.dp, top = 16.dp, end = 20.dp),
                )
                Tip(R.drawable.ic_computer, stringResource(R.string.tip_drag))
                Tip(R.drawable.ic_speaker, stringResource(R.string.tip_sound))
                Tip(R.drawable.ic_stats, stringResource(R.string.tip_bars))
                Spacer(Modifier.height(8.dp))
            }
        }
    }
}

@Composable
private fun Tip(@DrawableRes icon: Int, text: String) {
    ListItem(
        headlineContent = { Text(text, style = MaterialTheme.typography.bodyMedium) },
        leadingContent = { Icon(painterResource(icon), null, tint = MaterialTheme.colorScheme.primary) },
        colors = ListItemDefaults.colors(containerColor = Color.Transparent),
    )
}

@Composable
private fun Section(title: String, content: @Composable ColumnScope.() -> Unit) {
    Text(
        title,
        style = MaterialTheme.typography.labelLarge,
        color = MaterialTheme.colorScheme.primary,
        modifier = Modifier.padding(start = 8.dp, top = 12.dp),
    )
    Card(
        Modifier.fillMaxWidth(),
        shape = RoundedCornerShape(20.dp),
        colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surfaceContainerLow),
        content = content,
    )
}

@Composable
private fun SettingRow(
    icon: Painter,
    title: String,
    detail: String? = null,
    titleColor: Color = Color.Unspecified,
    onClick: (() -> Unit)? = null,
    trailing: @Composable (() -> Unit)? = null,
) {
    ListItem(
        headlineContent = { Text(title, color = titleColor) },
        supportingContent = detail?.let { { Text(it) } },
        leadingContent = { icon.Draw(tint = if (titleColor == Color.Unspecified) MaterialTheme.colorScheme.onSurfaceVariant else titleColor) },
        trailingContent = trailing,
        colors = ListItemDefaults.colors(containerColor = Color.Transparent),
        modifier = if (onClick != null) Modifier.clickable(onClick = onClick) else Modifier,
    )
}

@Composable
private fun SettingsScreen(state: UiState, actions: Actions) {
    BackHandler { actions.back() }
    var confirmForget by remember { mutableStateOf(false) }
    Scaffold(topBar = { PlainTopBar(stringResource(R.string.settings), actions::back) }) { padding ->
        Box(
            Modifier
                .fillMaxSize()
                .padding(padding)
                .verticalScroll(rememberScrollState()),
            contentAlignment = Alignment.TopCenter,
        ) {
            Column(
                Modifier
                    .widthIn(max = 640.dp)
                    .padding(horizontal = 24.dp, vertical = 8.dp),
                verticalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                Section(stringResource(R.string.section_streaming)) {
                    SettingRow(
                        DrawableIcon(R.drawable.ic_speaker),
                        stringResource(R.string.play_sound),
                        stringResource(R.string.play_sound_detail),
                        onClick = { actions.setPlaySound(!state.playSound) },
                    ) { Switch(state.playSound, actions::setPlaySound) }
                    HorizontalDivider(Modifier.padding(horizontal = 16.dp))
                    SettingRow(
                        DrawableIcon(R.drawable.ic_stats),
                        stringResource(R.string.show_stats),
                        stringResource(R.string.show_stats_detail),
                        onClick = { actions.setShowStats(!state.showStats) },
                    ) { Switch(state.showStats, actions::setShowStats) }
                }
                Section(stringResource(R.string.section_computer)) {
                    SettingRow(
                        DrawableIcon(R.drawable.ic_computer),
                        state.computerName ?: "",
                        stringResource(R.string.paired_computer),
                    )
                    HorizontalDivider(Modifier.padding(horizontal = 16.dp))
                    SettingRow(
                        VectorIcon(Icons.Filled.Add),
                        stringResource(R.string.pair_another),
                        onClick = actions::pairAnother,
                    )
                    HorizontalDivider(Modifier.padding(horizontal = 16.dp))
                    SettingRow(
                        VectorIcon(Icons.Filled.Delete),
                        stringResource(R.string.forget_computer, state.computerName ?: ""),
                        titleColor = MaterialTheme.colorScheme.error,
                        onClick = { confirmForget = true },
                    )
                }
                Section(stringResource(R.string.section_about)) {
                    SettingRow(VectorIcon(Icons.Filled.Info), stringResource(R.string.version), state.version)
                    HorizontalDivider(Modifier.padding(horizontal = 16.dp))
                    SettingRow(
                        DrawableIcon(R.drawable.ic_code),
                        stringResource(R.string.source_code),
                        stringResource(R.string.source_code_detail),
                        onClick = actions::openSource,
                    )
                    HorizontalDivider(Modifier.padding(horizontal = 16.dp))
                    SettingRow(
                        DrawableIcon(R.drawable.ic_coffee),
                        stringResource(R.string.donate),
                        stringResource(R.string.donate_detail),
                        onClick = actions::openDonate,
                    )
                }
                Spacer(Modifier.height(24.dp))
            }
        }
    }
    if (confirmForget) {
        AlertDialog(
            onDismissRequest = { confirmForget = false },
            icon = { Icon(Icons.Filled.Delete, null) },
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
