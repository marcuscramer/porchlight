package dev.porchlight.app.ui.theme

import androidx.compose.material3.LocalTextStyle as M3LocalTextStyle
import androidx.compose.material3.MaterialTheme as M3MaterialTheme
import androidx.compose.material3.darkColorScheme as m3DarkColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.drawWithCache
import androidx.compose.ui.draw.drawWithContent
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.BlendMode
import androidx.compose.ui.graphics.drawscope.scale
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.tv.material3.LocalTextStyle as TvLocalTextStyle
import androidx.tv.material3.MaterialTheme as TvMaterialTheme
import androidx.tv.material3.darkColorScheme as tvDarkColorScheme

// Color tokens are generated into GeneratedColor.kt, type numerics into
// GeneratedType.kt, spacing/shape/motion into Dimens.kt — all three by
// Style Dictionary from the shared tokens/ source (kept in sync with the
// web client); Type.kt keeps the one seam that can't be generated (which
// FontFamily object each role uses). This file just assembles them into
// two nested MaterialThemes: compose.material3 (outer) for OutlinedTextField/
// AlertDialog, which have no androidx.tv.material3 equivalent, and
// tv.material3 (inner) for everything else (Button/Switch/Text), which gets
// native per-state (focused/pressed/disabled) styling instead of hand-rolled
// focus tracking. Confirmed by reading tv.material3.MaterialTheme's own
// source that it provides entirely separate CompositionLocals from
// compose.material3's — nesting them doesn't shadow either one.

private val PorchlightColors = m3DarkColorScheme(
    primary = GeneratedColor.colorActionPrimaryBackground,
    onPrimary = GeneratedColor.colorActionPrimaryForeground,
    background = GeneratedColor.colorBackgroundApp,
    onBackground = GeneratedColor.colorTextPrimary,
    surface = GeneratedColor.colorBackgroundSurface,
    onSurface = GeneratedColor.colorTextPrimary,
    surfaceVariant = GeneratedColor.colorBackgroundSurfaceAlt,
    outline = GeneratedColor.colorBorderDefault,
    error = GeneratedColor.colorActionDangerBackground,
)

// tv.material3.ColorScheme's real field set (verified against the library's
// own published AAR/API surface, not assumed to mirror compose.material3):
// it has no `outline` — the equivalent is `border`/`borderVariant`.
private val PorchlightTvColors = tvDarkColorScheme(
    primary = GeneratedColor.colorActionPrimaryBackground,
    onPrimary = GeneratedColor.colorActionPrimaryForeground,
    background = GeneratedColor.colorBackgroundApp,
    onBackground = GeneratedColor.colorTextPrimary,
    surface = GeneratedColor.colorBackgroundSurface,
    onSurface = GeneratedColor.colorTextPrimary,
    surfaceVariant = GeneratedColor.colorBackgroundSurfaceAlt,
    error = GeneratedColor.colorActionDangerBackground,
    border = GeneratedColor.colorBorderDefault,
)

/**
 * The one background treatment behind every screen in the app now — a
 * single dim glow near the top-left corner, deliberately taller than it is
 * wide (unlike a screen's own actual aspect ratio), fading into the same
 * flat navy every screen used before this. Replaces bare
 * `.background(GeneratedColor.colorBackgroundWaiting)` calls one-for-one;
 * the flat color is still the base of every draw here; only the corner glow
 * is new. `drawWithCache` (not a plain `Brush.radialGradient` built once)
 * because the gradient's center/radius are fractions of this exact
 * composable's own size, which isn't known until layout — cached and only
 * rebuilt when that size actually changes, not on every recomposition.
 * `scale(scaleX = 0.55f, ...)` is what actually makes the glow taller than
 * wide: Compose's `radialGradient` only ever draws a true circle, so this
 * squeezes the canvas horizontally around the glow's own center before
 * drawing it, then un-squeezes for everything drawn after — the same trick
 * as CSS's own `ellipse Wpx Hpx at X% Y%` gradient shorthand, just spelled
 * out by hand since Compose has no elliptical-gradient primitive.
 */
fun Modifier.porchlightScreenBackground(): Modifier = this.drawWithCache {
    val center = Offset(size.width * 0.22f, size.height * -0.08f)
    val radius = size.height * 0.85f
    val glow = Brush.radialGradient(
        colors = listOf(GeneratedColor.colorBackgroundGlow, GeneratedColor.colorBackgroundWaiting),
        center = center,
        radius = radius,
    )
    onDrawBehind {
        drawRect(color = GeneratedColor.colorBackgroundWaiting)
        scale(scaleX = 0.55f, scaleY = 1f, pivot = center) {
            drawCircle(brush = glow, radius = radius, center = center)
        }
    }
}

/**
 * Fades the top and bottom edge of a vertically-scrollable `Column` —
 * unconditionally now, not gated on the scroll state's own canScrollBackward/
 * canScrollForward (explicit request; mirrors web's identical
 * .contact-list mask-image, styles.css, control-for-control — see its own
 * doc for the full reasoning this mirrors). A permanent fade still only
 * ever visibly dissolves something when there's real content to hide,
 * without tracking scroll position at all: the caller is expected to put
 * an empty `Spacer(Modifier.height(topHeight/bottomHeight))` as the first/
 * last child of the scrollable content (WaitingScreen does, for its
 * contact list) — at rest, that spacer alone occupies the fade zone, so it
 * dissolves empty space, not content; scroll past it and real content
 * takes its place there instead.
 *
 * Bottom is always one plain single linear fade across its own full
 * height. Top can be either that same plain shape, or — when [topPlain] is
 * false — a split shape instead: a flat, already-fully-transparent outer
 * half plus a compressed inner ramp, which exists to keep row text clear
 * of the settings gear overlaid in that corner on a screen where a
 * straight fade wouldn't get transparent enough soon enough. Callers
 * should only pass `topPlain = true` once they've measured real,
 * live clearance between the scrollable content and whatever floats in
 * that corner, not assumed it for a specific screen size — Porchlight
 * isn't guaranteed to run only on one Portal model (see WaitingScreen's
 * own call site, which measures this live via onGloballyPositioned rather
 * than hardcoding either shape) — mirrors web's identical resize-driven
 * live check for the same reason (`.contact-list` mask-image, styles.css).
 *
 * `graphicsLayer(alpha = 0.99f)` forces this subtree onto its own
 * compositing layer, which [BlendMode.DstIn] needs in order to only affect
 * this content rather than everything drawn behind it.
 */
fun Modifier.fadingEdges(topHeight: Dp, bottomHeight: Dp, topPlain: Boolean): Modifier = this
    .graphicsLayer(alpha = 0.99f)
    .drawWithContent {
        drawContent()
        val topPx = topHeight.toPx()
        drawRect(
            brush = if (topPlain) {
                Brush.verticalGradient(0f to Color.Transparent, 1f to Color.Black, startY = 0f, endY = topPx)
            } else {
                Brush.verticalGradient(
                    0f to Color.Transparent, 0.5f to Color.Transparent, 1f to Color.Black,
                    startY = 0f, endY = topPx,
                )
            },
            blendMode = BlendMode.DstIn,
        )
        val bottomPx = bottomHeight.toPx()
        drawRect(
            brush = Brush.verticalGradient(
                0f to Color.Black, 1f to Color.Transparent,
                startY = size.height - bottomPx, endY = size.height,
            ),
            blendMode = BlendMode.DstIn,
        )
    }

@Composable
fun PorchlightTheme(content: @Composable () -> Unit) {
    M3MaterialTheme(
        colorScheme = PorchlightColors,
        typography = PorchlightTypography,
    ) {
        TvMaterialTheme(
            colorScheme = PorchlightTvColors,
            typography = PorchlightTvTypography,
        ) {
            // Make Inter the baseline font for every Text in both libraries'
            // text-style locals, so bare `Text(...)` calls inherit the type
            // face and don't fall back to the system sans. Deliberately NOT
            // baking a color into this merged style (an earlier version did,
            // via Type.body.copy(color = ...)) — a hardcoded color here
            // would apply to every Text unconditionally, including inside a
            // focused TvButton, permanently overriding the *content* color
            // tv.material3's Button already flips to contrast against its
            // own focused (light) background. Confirmed as a real, live bug
            // this way: a default-styled TvButton's label text was
            // literally invisible (white-on-white) the instant it gained
            // focus, on every screen that uses one (AdminChoiceScreen's
            // "Rename this device"/"Contacts", NameEntryScreen's
            // "Continue", ...) — never caught earlier because on-device
            // D-pad focus testing had been abandoned for lack of hardware/
            // a working emulator (see the tv.material3 migration's own
            // notes). Leaving color unspecified here lets each Text fall
            // through to LocalContentColor instead, which already defaults
            // to colorTextPrimary at the theme root (via onSurface) for
            // ordinary text, and correctly flips per-state inside a Button.
            val m3Base = M3LocalTextStyle.current.merge(Type.body)
            val tvBase = TvLocalTextStyle.current.merge(Type.body)
            CompositionLocalProvider(M3LocalTextStyle provides m3Base) {
                CompositionLocalProvider(TvLocalTextStyle provides tvBase, content = content)
            }
        }
    }
}
