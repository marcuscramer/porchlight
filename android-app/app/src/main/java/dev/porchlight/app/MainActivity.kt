package dev.porchlight.app

import android.Manifest
import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.content.ServiceConnection
import android.os.Build
import android.os.Bundle
import android.os.Handler
import android.os.IBinder
import android.os.Looper
import android.os.SystemClock
import android.view.KeyEvent
import android.view.WindowManager
import androidx.activity.ComponentActivity
import androidx.activity.compose.BackHandler
import androidx.activity.compose.setContent
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.interaction.collectIsFocusedAsState
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.ArrowForward
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Surface
import androidx.compose.material3.Text as M3Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusProperties
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.focus.onFocusChanged
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.input.key.Key
import androidx.compose.ui.input.key.KeyEventType
import androidx.compose.ui.input.key.key
import androidx.compose.ui.input.key.onPreviewKeyEvent
import androidx.compose.ui.input.key.type
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.style.TextAlign
import androidx.tv.material3.Border
import androidx.tv.material3.ClickableSurfaceDefaults
import androidx.tv.material3.ClickableSurfaceScale
import androidx.tv.material3.Icon
import androidx.tv.material3.MaterialTheme
import androidx.tv.material3.ProvideTextStyle
import androidx.tv.material3.Text
import androidx.tv.material3.Surface as TvSurface
import dev.porchlight.app.ui.theme.Dimens
import dev.porchlight.app.ui.theme.GeneratedColor
import dev.porchlight.app.ui.theme.GeneratedOpacity
import dev.porchlight.app.ui.theme.GeneratedType
import dev.porchlight.app.ui.theme.PorchlightTheme
import dev.porchlight.app.ui.theme.porchlightScreenBackground
import com.vitorpamplona.quartz.nip01Core.core.toHexKey
import com.vitorpamplona.quartz.nip01Core.crypto.KeyPair
import java.util.UUID
import kotlinx.coroutines.delay
import androidx.compose.animation.core.animateDpAsState
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.ui.draw.clip
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.role
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.toggleableState
import androidx.compose.ui.state.ToggleableState
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.semantics.stateDescription

/**
 * Closes a real, unresolved upstream Compose bug
 * (issuetracker.google.com/issues/374031296): a *hardware* Enter/DPAD_CENTER
 * key press (not a soft-keyboard IME action tap) submitting a text field is
 * a single physical press, but Android dispatches its KeyDown and KeyUp as
 * two *separate* events. Consuming the KeyDown only stops *that* event from
 * propagating — the KeyUp is dispatched independently, a moment later, to
 * whatever view is focused by then. Since submitting navigates to a new
 * screen in the same stroke, that's the new screen's own first real
 * focusable action button (Call, Accept) — and Android's own default
 * View-level key handling performs a real click on KEYCODE_ENTER/
 * DPAD_CENTER's key-up if nothing else handles it. Concretely: renaming
 * this device this way could place a real, unwanted call.
 *
 * `arm()` marks "the next Enter/DPAD_CENTER key-up is a known stray from
 * an already-handled KeyDown, not a real user action" — called immediately
 * after `submit()` succeeds. `MainActivity.dispatchKeyEvent` (the one place
 * that sees every key event before *any* View, Compose's own focused node
 * included) checks this and swallows exactly that one key-up, system-wide,
 * regardless of which screen or button ends up focused by the time it
 * arrives. One-shot by design, with a generous safety timeout in case a
 * KeyUp is ever lost outright (a stuck flag would otherwise silently eat a
 * future, entirely unrelated Enter press).
 */
internal object HardwareEnterKeyUpGuard {
    private var armedAtMs: Long = 0L
    private const val MAX_AGE_MS = 2000L

    fun arm() { armedAtMs = SystemClock.uptimeMillis() }

    /** True at most once per [arm] call — disarms itself either way. */
    fun consumeIfArmed(): Boolean {
        val was = armedAtMs != 0L && SystemClock.uptimeMillis() - armedAtMs < MAX_AGE_MS
        armedAtMs = 0L
        return was
    }
}

/**
 * Minimal multi-contact calling shell: local self-view (small, movable
 * corner), remote video (full-screen once a peer connects), and a setup
 * flow entered once on first run (name, then "Add contact" — see
 * PassphrasePairingScreens.kt) that stays reachable afterward too — Add
 * contact via its own row in the contact list, renaming via the settings
 * gear icon — since adding more contacts and recovering one after a
 * suspected compromise (Delete, then Add contact again) are both things
 * this device needs to support long after first setup, not just once.
 */
class MainActivity : ComponentActivity() {

    /** See [HardwareEnterKeyUpGuard]'s own doc. */
    override fun dispatchKeyEvent(event: KeyEvent): Boolean {
        val isEnterUp = event.action == KeyEvent.ACTION_UP &&
            (event.keyCode == KeyEvent.KEYCODE_ENTER || event.keyCode == KeyEvent.KEYCODE_NUMPAD_ENTER || event.keyCode == KeyEvent.KEYCODE_DPAD_CENTER)
        if (isEnterUp && HardwareEnterKeyUpGuard.consumeIfArmed()) return true
        return super.dispatchKeyEvent(event)
    }

    private var service by mutableStateOf<CameraAgentService?>(null)
    private var showAdminChoice by mutableStateOf(false)

    private val connection = object : ServiceConnection {
        override fun onServiceConnected(name: ComponentName?, binder: IBinder?) {
            service = (binder as? CameraAgentService.LocalBinder)?.service
        }
        override fun onServiceDisconnected(name: ComponentName?) { service = null }
    }

    private val permissionLauncher = registerForActivityResult(
        ActivityResultContracts.RequestMultiplePermissions()
    ) { /* user can retry Connect if denied */ }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        requestPermissions()
        setContent {
            PorchlightTheme {
                Surface(modifier = Modifier.fillMaxSize().porchlightScreenBackground(), color = Color.Transparent) {
                    AppRoot(
                        service = service,
                        showAdminChoice = showAdminChoice,
                        onAdminChoiceHandled = { showAdminChoice = false },
                        onReopenAdminChoice = { showAdminChoice = true },
                        onConnect = { cfg -> Config.save(this, cfg); CameraAgentService.start(this) },
                    )
                }
            }
        }
    }

    override fun onStart() {
        super.onStart()
        bindService(Intent(this, CameraAgentService::class.java), connection, Context.BIND_AUTO_CREATE)
    }

    override fun onStop() {
        super.onStop()
        try { unbindService(connection) } catch (_: Exception) {}
    }

    /**
     * Fires specifically for a deliberate user navigation away (Home, or
     * switching to another app) — unlike onPause/onStop, it does NOT fire
     * for the screensaver taking over or other transient interruptions.
     * That distinction matters: tying hangup to onStop instead would undo
     * the whole no-screensaver-during-calls fix by treating a dream taking
     * over the same as the user actually leaving. Hang-up-only, not a full
     * stop — see hangUp()'s doc — so this can never strand your parents'
     * device unreachable if Home gets pressed by accident.
     */
    override fun onUserLeaveHint() {
        super.onUserLeaveHint()
        service?.onUserLeaveHint()
    }

    private fun requestPermissions() {
        val perms = mutableListOf(Manifest.permission.CAMERA, Manifest.permission.RECORD_AUDIO)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            perms.add(Manifest.permission.POST_NOTIFICATIONS)
        }
        permissionLauncher.launch(perms.toTypedArray())
    }
}

/**
 * What a [TvButton] means, not what color it is — [Neutral] for every
 * ordinary action (Continue, Confirm, Rename, Add contact, Try again, Call
 * again, ...; these have no shared meaning beyond "the button on this
 * screen"), [Success] for the one genuinely positive/"go" action (Call,
 * Accept — already `colorStatusOk` green in this app before this), [Danger]
 * for a genuinely destructive or rejecting one (Delete, Decline — already
 * `colorActionDangerBackground` red before this). All three render as a
 * translucent tint over whatever's behind the button (see [TvButton]'s own
 * doc), not a solid fill — color now marks *meaning*, not just "this is a
 * button," so a screen with one Neutral action and nothing else doesn't
 * read as any louder than it needs to.
 */
internal enum class TvButtonTint { Neutral, Success, Danger }

/**
 * A Button whose color is a translucent tint of its [tint]'s meaning,
 * brightening (not glowing outward) on focus. Backed directly by
 * androidx.tv.material3's `Surface` rather than `Button`: `Button` bakes in
 * its own internal `Modifier.defaultMinSize(58.dp, 40.dp)` on the Row
 * wrapping its content, applied to a fresh `Modifier` local to `Button`'s
 * own implementation, not reachable through this function's own `modifier`
 * parameter (confirmed via javap on ButtonKt.class) — so every `Button` was
 * floored at 40dp tall regardless of its own padding tokens. `Surface` has
 * no such internal minimum, so building the content Row ourselves here lets
 * height come from content + padding alone, matching web's proportions —
 * still using `Surface`'s native per-state border/color/scale rather than
 * hand-tracking `onFocusChanged`, since focused/pressed/disabled are
 * first-class states there.
 *
 * Every state (unfocused fill, unfocused border, focused fill, focused
 * border) is an alpha-blended version of one single hue per [tint] — focus
 * here means "more of the same color," solid and contained within the
 * button's own edge, not a glow extending past it — a hard, high-contrast
 * border reads far more reliably on a TV panel than a soft outer shadow.
 * [Neutral]'s tint is a plain white overlay rather than a named hue, since
 * "no strong meaning" has no color to draw from.
 *
 * Shape/padding come from the shared design tokens (`Dimens`) rather than
 * either library's own unpinned defaults, so a Compose library bump can't
 * silently drift this from the web client's matching `.tv-button` rule.
 */
// Not top-level vals: ClickableSurfaceDefaults.border/colors are @Composable
// @ReadOnlyComposable (they read the current theme), so they can only be
// called from within a composable — TvButtonShape is the one plain value
// here, since RoundedCornerShape itself isn't theme-dependent.
private val TvButtonShape = RoundedCornerShape(Dimens.buttonRadius)

@Composable
internal fun TvButton(
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
    enabled: Boolean = true,
    tint: TvButtonTint = TvButtonTint.Neutral,
    // Exposed so a caller can observe this button's own focus state
    // externally (e.g. brightening a label elsewhere on the same row) —
    // same pattern WaitingScreen's own Auto-answer Switch already uses.
    interactionSource: MutableInteractionSource? = null,
    // A smaller button (label and padding), for a secondary action on a page of text rows.
    compact: Boolean = false,
    content: @Composable RowScope.() -> Unit,
) {
    val hue = when (tint) {
        TvButtonTint.Neutral -> Color.White
        TvButtonTint.Success -> GeneratedColor.colorStatusOk
        TvButtonTint.Danger -> GeneratedColor.colorActionDangerBackground
    }
    val containerAlpha = if (tint == TvButtonTint.Neutral) GeneratedOpacity.buttonTintNeutralContainer else GeneratedOpacity.buttonTintAccentContainer
    val focusedContainerAlpha = if (tint == TvButtonTint.Neutral) GeneratedOpacity.buttonTintNeutralContainerFocused else GeneratedOpacity.buttonTintAccentContainerFocused
    val borderAlpha = if (tint == TvButtonTint.Neutral) GeneratedOpacity.buttonTintNeutralBorder else GeneratedOpacity.buttonTintAccentBorder
    val focusedBorderAlpha = if (tint == TvButtonTint.Neutral) GeneratedOpacity.buttonTintNeutralBorderFocused else GeneratedOpacity.buttonTintAccentBorderFocused
    val contentColor = if (tint == TvButtonTint.Neutral) GeneratedColor.colorTextPrimary else hue

    // ClickableSurfaceDefaults' own focused/unfocused color swap is driven
    // by tv-material3's own internal focus tracking, which can silently
    // miss a button gaining focus in the same beat its screen is first
    // mounted (an on-mount auto `focusRequester.requestFocus()`, rather
    // than a D-pad move between two already-settled buttons): the node
    // genuinely holds focus and responds to input (confirmed live via
    // `onFocusChanged`, which always fired correctly), yet the surface
    // keeps painting its *unfocused* colors indefinitely (found live
    // 2026-10-01, a D-pad-driven delete-confirmation button stuck this way
    // with no later frame ever correcting it — reproduced consistently,
    // independent of how many times or how long after mount focus was
    // re-requested). Rather than depend on whichever internal signal
    // tv-material3 uses to pick a color, `isReallyFocused` is our own
    // ground truth from the plain, synchronous `onFocusChanged` callback,
    // and both the "focused" and "unfocused" slots below resolve to
    // whatever color *that* says is correct — so no matter which slot the
    // library actually paints, it paints the right one.
    //
    // Gated on `enabled` too: a disabled surface can still receive D-pad
    // focus (same underlying unreliable focus/color wiring as above), and
    // without this a disabled button would brighten to its focused colors
    // like it was interactive, which it isn't. A disabled button's colors
    // stay pinned to the baseline regardless of focus, and its content is
    // muted further still (a flat alpha cut, not a separate color — reads
    // correctly against any tint) so "disabled" is unambiguous at a glance.
    var isReallyFocused by remember { mutableStateOf(false) }
    val resolvedIsFocused = enabled && isReallyFocused
    val resolvedContainerAlpha = if (resolvedIsFocused) focusedContainerAlpha else containerAlpha
    val resolvedBorderAlpha = if (resolvedIsFocused) focusedBorderAlpha else borderAlpha
    val resolvedContentColor = if (enabled) contentColor else contentColor.copy(alpha = GeneratedOpacity.opacity50)

    TvSurface(
        onClick = onClick,
        enabled = enabled,
        modifier = modifier.onFocusChanged { isReallyFocused = it.isFocused },
        interactionSource = interactionSource,
        // None, same as Button's own scale = ButtonScale.None used to be —
        // tv.material3's own default grows a focused surface ~10% larger,
        // and with several buttons sitting close together on a contact row,
        // that growth clipped against a neighboring row/the scrollable
        // list's own bounds. Also contradicts this function's own
        // documented design above — focus here is meant to read entirely
        // through color, never size.
        scale = ClickableSurfaceScale.None,
        shape = ClickableSurfaceDefaults.shape(shape = TvButtonShape),
        border = ClickableSurfaceDefaults.border(
            border = Border(border = BorderStroke(Dimens.borderWidthDefault, hue.copy(alpha = resolvedBorderAlpha)), shape = TvButtonShape),
            focusedBorder = Border(border = BorderStroke(Dimens.borderWidthDefault, hue.copy(alpha = resolvedBorderAlpha)), shape = TvButtonShape),
            disabledBorder = Border(border = BorderStroke(Dimens.borderWidthDefault, hue.copy(alpha = borderAlpha)), shape = TvButtonShape),
            focusedDisabledBorder = Border(border = BorderStroke(Dimens.borderWidthDefault, hue.copy(alpha = borderAlpha)), shape = TvButtonShape),
        ),
        colors = ClickableSurfaceDefaults.colors(
            containerColor = hue.copy(alpha = resolvedContainerAlpha),
            contentColor = resolvedContentColor,
            focusedContainerColor = hue.copy(alpha = resolvedContainerAlpha),
            focusedContentColor = resolvedContentColor,
            disabledContainerColor = hue.copy(alpha = containerAlpha),
            disabledContentColor = contentColor.copy(alpha = GeneratedOpacity.opacity50),
        ),
    ) {
        // Mirrors what Button's own internal content Row does (Arrangement.
        // Center, CenterVertically, contentPadding, ProvideTextStyle(
        // typography.labelLarge)) — without the last one, any text content
        // would fall back to whatever LocalTextStyle happens to be ambient
        // at each call site instead of this button's own fixed label style.
        // Regular weight, not the token's own Medium — a local override
        // here rather than touching GeneratedType (shared with the web
        // client's own button styling via the same token source).
        val labelStyle = if (compact) MaterialTheme.typography.labelSmall else MaterialTheme.typography.labelLarge
        ProvideTextStyle(labelStyle.copy(fontWeight = FontWeight(GeneratedType.fontWeightRegular))) {
            Row(
                horizontalArrangement = Arrangement.Center,
                verticalAlignment = Alignment.CenterVertically,
                modifier = Modifier.padding(
                    horizontal = if (compact) Dimens.dimension10 else Dimens.buttonPaddingBaseHorizontal,
                    vertical = if (compact) Dimens.dimension4 else Dimens.buttonPaddingBaseVertical,
                ),
                content = content,
            )
        }
    }
}

/**
 * An on/off switch: [FocusableStepSlider] with two positions, at the stock
 * switch's own size (40 x 24dp). Built from that rather than tv.material3's
 * [androidx.tv.material3.Switch] because the stock one enlarges its thumb
 * when checked (and has no focused-state colors at all), and this app wants
 * the knob the same size in both positions and no border in any state.
 * Focus brightens the fill, exactly as it does for every neutral button.
 */
@Composable
internal fun FocusableSwitch(
    checked: Boolean,
    onCheckedChange: (Boolean) -> Unit,
    modifier: Modifier = Modifier,
    interactionSource: MutableInteractionSource = remember { MutableInteractionSource() },
) {
    FocusableStepSlider(
        steps = 2,
        selected = if (checked) 1 else 0,
        onSelectedChange = { onCheckedChange(it == 1) },
        trackWidth = Dimens.dimension40,
        interactionSource = interactionSource,
        modifier = modifier.semantics { role = Role.Switch; toggleableState = ToggleableState(checked) },
    )
}

/**
 * A long, toggle-looking control with [steps] discrete positions: the knob
 * sits at [selected], and everything to its left is filled green exactly like
 * a [FocusableSwitch] that's on (the first position reads as "off" — no
 * fill). It has no border in any state; focus brightens the fill the same way
 * it does for a switch.
 * Click cycles forward (wrapping); callers handle Left/Right themselves via
 * [modifier].
 */
@Composable
internal fun FocusableStepSlider(
    steps: Int,
    selected: Int,
    onSelectedChange: (Int) -> Unit,
    modifier: Modifier = Modifier,
    trackWidth: Dp = Dimens.dimension180 * 0.6f,
    interactionSource: MutableInteractionSource = remember { MutableInteractionSource() },
) {
    val focused by interactionSource.collectIsFocusedAsState()
    // Height, knob size and knob inset all match the stock switch beside it
    // (Kiosk mode), measured off its off position: 24dp track, 12dp knob,
    // 6dp gap all round.
    val trackHeight = Dimens.dimension24
    val knobSize = Dimens.dimension12
    val inset = Dimens.dimension6
    val shape = CircleShape
    val on = selected > 0
    val neutralContainerAlpha = if (focused) GeneratedOpacity.buttonTintNeutralContainerFocused else GeneratedOpacity.buttonTintNeutralContainer
    val fillAlpha = if (focused) GeneratedOpacity.buttonTintAccentBorderFocused else GeneratedOpacity.buttonTintAccentContainerFocused
    val knobOffset by animateDpAsState(
        inset + (trackWidth - knobSize - inset * 2) * selected / (steps - 1).coerceAtLeast(1),
        label = "stepSliderKnob",
    )
    Box(
        modifier = modifier
            .width(trackWidth)
            .height(trackHeight)
            .clip(shape)
            .background(Color.White.copy(alpha = neutralContainerAlpha))
            .clickable(interactionSource = interactionSource, indication = null) {
                onSelectedChange((selected + 1) % steps)
            },
    ) {
        // The green fill, from the left edge to one knob-inset past the
        // knob, with a rounded end — the same margin the knob has to the
        // track's end at the last position, where this is simply the whole
        // track.
        if (on) {
            Box(
                modifier = Modifier
                    .fillMaxHeight()
                    .width(knobOffset + knobSize + inset)
                    .clip(shape)
                    .background(GeneratedColor.colorStatusOk.copy(alpha = fillAlpha)),
            )
        }
        Box(
            modifier = Modifier
                .align(Alignment.CenterStart)
                .offset(x = knobOffset)
                .size(knobSize)
                .clip(CircleShape)
                .background(if (on) GeneratedColor.colorTextPrimary else GeneratedColor.colorTextDim),
        )
    }
}

/**
 * Shared by every screen that auto-dismisses itself after showing a purely
 * informational, nothing-left-to-decide prompt (call outcomes, pairing
 * collision/timeout) — one constant instead of a separate copy per file, now
 * that [CenteredDialogScreen] centralizes the shell these all render inside
 * too. Deliberately NOT used by a dialog with a real decision to make
 * (delete-confirmation, the pairing name-confirm tap) — those stay up until
 * the person acts or backs out.
 */
internal const val AUTO_DISMISS_DELAY_MS = GeneratedSharedConfig.AUTO_DISMISS_DELAY_MS

/** How long the ring-volume setting must sit unchanged before the chime
 * previews it — long enough that cycling through the levels stays silent. */
private const val RING_VOLUME_PREVIEW_DELAY_MS = 800L

/**
 * Claims focus for a just-entered screen's one focusable element — one
 * named place for a pattern that was previously duplicated verbatim across
 * every such screen. See [TvButton]'s own `isReallyFocused` doc for the
 * related, separate bug this is *not* responsible for: focus itself lands
 * here reliably (confirmed live), the thing that could silently fail was
 * the button's *visual* focused state, not the focus request itself.
 */
@Composable
internal fun RequestFocusOnMount(focusRequester: FocusRequester) {
    LaunchedEffect(Unit) { focusRequester.requestFocus() }
}

/**
 * The shared "centered dialog" shape behind every bright-title, one-thing-
 * to-say prompt in the app: call outcomes, pairing collision/timeout,
 * delete-confirmation, the pairing name-confirm tap, and the "waiting for
 * the other device" spinner. [PageScreen] is the contrasting family — a
 * page-like "go manage something" destination (dim, upper-left title)
 * rather than a moment. `widthIn(max = sizeMaxContentWidth)` matters here
 * for the same reason it does on [PageScreen]: a long outcome message
 * (e.g. "The call with X never connected. Check that...") is otherwise free
 * to demand however much of the screen's own width it wants — this doesn't
 * currently misrender without the cap (none of these use [PageScreen]'s own
 * `width(IntrinsicSize.Max)` mechanism, the specific thing that broke on
 * `EnterPhraseScreen`), but capping it here too is cheap, future-proof
 * insurance, and matches first-time [NameEntryScreen]'s own dialog branch,
 * which already used this same cap independently.
 */
@Composable
internal fun CenteredDialogScreen(onBack: () -> Unit, content: @Composable ColumnScope.() -> Unit) {
    BackHandler(onBack = onBack)
    Box(modifier = Modifier.fillMaxSize().porchlightScreenBackground(), contentAlignment = Alignment.Center) {
        Column(
            horizontalAlignment = Alignment.CenterHorizontally,
            verticalArrangement = Arrangement.spacedBy(Dimens.spacingPanelContentGap),
            modifier = Modifier.widthIn(max = Dimens.sizeMaxContentWidth).padding(horizontal = Dimens.spacingScreenPadding),
            content = content,
        )
    }
}

/**
 * The shared "page" shape behind every settings-gear-reachable screen that
 * isn't a centered dialog: a dim, upper-left title (this is "which screen am
 * I on," not content the user came here to read — see [OutcomeScreen]'s own
 * doc for the contrasting bright-title/centered-dialog family) with the rest
 * of the content centered in a fixed-max-width block in the remaining space.
 * [Dimens.sizeMaxContentWidth] is the same token the first-time
 * [NameEntryScreen] dialog already uses for this — a fixed cap applied
 * uniformly, not "hug whatever width my widest child happens to want." That
 * distinction matters: an earlier version of this composable used
 * `width(IntrinsicSize.Max)` instead, which worked for Settings'/Rename's
 * short, narrow content but broke on `EnterPhraseScreen`'s long wrapping
 * paragraphs — a paragraph's intrinsic (single-line, unwrapped) width is
 * huge, so the column just got clamped to the full screen width with no
 * side margins, stranding its button off-center.
 *
 * `horizontalAlignment = CenterHorizontally` is a deliberate no-op for a
 * fillMaxWidth child (Settings' own table rows, any text field) — it only
 * has a visible effect on a child that isn't already stretched to the full
 * width, i.e. a compact submit/save button, which is exactly what should
 * center: matches the same centering convention [OutcomeScreen] and the
 * first-time [NameEntryScreen] dialog already use, rather than inventing a
 * second "how do we center things" rule just for this family of screens. A
 * prose paragraph needs its own explicit `fillMaxWidth()` +
 * `textAlign = TextAlign.Center` to actually center each wrapped line (a
 * non-fillMaxWidth Text's bounding box centers as a whole, but its text
 * stays left-aligned within that box) — this alone doesn't do that for you.
 *
 * [content]'s own column gets a scroll modifier for free, applied uniformly
 * rather than only where someone happened to add it (previously just
 * EnterPhraseScreen) — harmless when content already fits the space, a real
 * safety net when it doesn't.
 */
@Composable
internal fun PageScreen(title: String, content: @Composable ColumnScope.() -> Unit) {
    Column(modifier = Modifier.fillMaxSize().padding(Dimens.spacingScreenPadding)) {
        Text(title, color = GeneratedColor.colorTextDim, style = MaterialTheme.typography.headlineSmall)
        Box(modifier = Modifier.weight(1f).fillMaxWidth(), contentAlignment = Alignment.Center) {
            Column(
                modifier = Modifier.widthIn(max = Dimens.sizeMaxContentWidth).fillMaxWidth().verticalScroll(rememberScrollState()),
                horizontalAlignment = Alignment.CenterHorizontally,
                verticalArrangement = Arrangement.spacedBy(Dimens.spacingPanelContentGap),
                content = content,
            )
        }
    }
}

/**
 * A single-line, Enter-submits [OutlinedTextField] — the same
 * `onPreviewKeyEvent` + [HardwareEnterKeyUpGuard.arm] dance duplicated
 * verbatim between `NameEntryScreen` and `EnterPhraseScreen` before this was
 * extracted (see [HardwareEnterKeyUpGuard]'s own doc for why `arm()` is
 * needed in addition to handling the KeyDown here, not instead of it).
 */
@Composable
internal fun SubmitOnEnterTextField(
    value: String,
    onValueChange: (String) -> Unit,
    label: String,
    onSubmit: () -> Unit,
    modifier: Modifier = Modifier,
) {
    OutlinedTextField(
        value = value,
        onValueChange = onValueChange,
        label = { M3Text(label) },
        modifier = modifier.onPreviewKeyEvent { event ->
            if (event.type == KeyEventType.KeyDown && (event.key == Key.Enter || event.key == Key.NumPadEnter)) {
                onSubmit()
                HardwareEnterKeyUpGuard.arm()
                true
            } else {
                false
            }
        },
        // singleLine forces the IME to offer a Done action instead of a
        // newline key for Enter/Return — without it, Enter just inserts
        // "\n" into the field, with no way to submit from the keyboard.
        singleLine = true,
        keyboardOptions = KeyboardOptions(imeAction = ImeAction.Done),
        keyboardActions = KeyboardActions(onDone = { onSubmit() }),
    )
}

internal fun PreviewCorner.toAlignment(): Alignment = when (this) {
    PreviewCorner.TOP_START -> Alignment.TopStart
    PreviewCorner.TOP_END -> Alignment.TopEnd
    PreviewCorner.BOTTOM_START -> Alignment.BottomStart
    PreviewCorner.BOTTOM_END -> Alignment.BottomEnd
    // Never actually resolved — HomeScreen's call view skips composing the
    // preview Box entirely for INVISIBLE (see its own doc) rather than
    // aligning it somewhere and hiding it. A harmless fallback, only here
    // so this `when` stays exhaustive.
    PreviewCorner.INVISIBLE -> Alignment.BottomStart
}

/**
 * Every screen reachable from the settings gear icon, plus the "Add
 * contact" flow reached directly from WaitingScreen's own contact list now
 * (not from Settings at all — see EnteringPhrase's own doc). Kept as one
 * sealed type rather than a pile of booleans. Deliberately local
 * (`remember`) state inside AppRoot, not lifted to MainActivity like
 * `showAdminChoice` is — nothing outside AppRoot's own composition ever
 * needs to read or set which of these screens is showing.
 */
private sealed interface AdminScreen {
    // Only ever reached via `showAdminChoice` (the settings gear icon), so
    // Back/Save always fall through to reopening Device settings — no
    // second entry point to distinguish, unlike before this screen's own
    // Contacts button was removed.
    data object Rename : AdminScreen
    // Read-only debug page reached from Settings; Back reopens Settings.
    data object ConnectionInfo : AdminScreen
    // "Add contact" only — there's no "Reconnect": a stale contact is just
    // Delete + Add contact again, so this carries no pairing id at all;
    // every attempt is brand new. Reached directly from a
    // trailing row in WaitingScreen's own contact list (both once this
    // device already has contacts, and pre-onboarding with zero — the
    // waiting screen renders either way, empty list and all). Per-contact
    // management (Delete, auto-answer) lives right on each ContactRow now,
    // not behind a separate Contacts screen. Exactly one entry point means
    // Back always falls straight back to the waiting screen.
    data object EnteringPhrase : AdminScreen
    // A phrase was submitted — waiting on the SPAKE2 exchange to resolve
    // (candidate found, collision, or timeout). See PairingProgressScreen's
    // doc. The attempt exists only in memory (CameraAgentService's
    // pairingAttempt, backed by call-core), so cancelling or abandoning it via
    // a retry leaves nothing behind.
    data object PairingInProgress : AdminScreen
}

@Composable
private fun AppRoot(
    service: CameraAgentService?,
    showAdminChoice: Boolean,
    onAdminChoiceHandled: () -> Unit,
    onReopenAdminChoice: () -> Unit,
    onConnect: (Config) -> Unit,
) {
    val context = LocalContext.current
    var config by remember { mutableStateOf(Config.load(context)) }
    // Always changes the freshly loaded config, never this composable's own
    // (possibly stale) snapshot: CameraAgentService can have pinned a peer
    // directly to disk since `config` was last refreshed here, and saving a
    // stale copy would silently discard that pin.
    fun updateConfig(change: (Config) -> Config) {
        val c = change(Config.load(context))
        Config.save(context, c)
        config = c
    }
    val state by (service?.state?.collectAsState() ?: remember { mutableStateOf(CameraAgentService.AgentState()) })
    var adminScreen by remember { mutableStateOf<AdminScreen?>(null) }

    // Keep the screen on while a call is active so it can't sleep mid-call.
    // Keyed on activePairingId (same signal HomeScreens.kt's own
    // activeContact uses), not state.running — found live on real Portal
    // hardware that running stays true the entire time the background
    // service is connected to signaling, not just during a call, which
    // kept the screen (and Immortal's screensaver) from ever going idle
    // while just sitting on the waiting screen.
    //
    // Also, while activePairingId is set, claim the three window flags that
    // tell the system this window itself belongs above the dream/keyguard
    // (SHOW_WHEN_LOCKED/TURN_SCREEN_ON/DISMISS_KEYGUARD) — found live that
    // CameraAgentService's setFullScreenIntent reliably *launches* and
    // resumes MainActivity from deep sleep (confirmed in logcat), but
    // without these flags the Portal's own dream service (SuperframeDream)
    // independently re-engages ~180ms later and pauses it right back off
    // screen — a real race *after* the launch, not a launch-priority race.
    // These flags are what actually keep the window on top once it's up.
    LaunchedEffect(state.activePairingId) {
        val flags = WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON or
            WindowManager.LayoutParams.FLAG_SHOW_WHEN_LOCKED or
            WindowManager.LayoutParams.FLAG_TURN_SCREEN_ON or
            WindowManager.LayoutParams.FLAG_DISMISS_KEYGUARD
        (context as? android.app.Activity)?.window?.let {
            if (state.activePairingId != null) it.addFlags(flags) else it.clearFlags(flags)
        }
    }

    // `config` only reflects whatever was last explicitly (re)loaded into
    // it, not the live CameraAgentService state — a peer confirmed via
    // WaitingScreen's own row tap never touches this `config` variable, so
    // it stays stale until something reloads it. Reload here too, so
    // opening Settings can't show a stale name/pairing list.
    LaunchedEffect(showAdminChoice) {
        if (showAdminChoice) config = Config.load(context)
    }

    // See CameraAgentService.startPairing: the service hands the passphrase to
    // itself when it isn't running yet (the first pairing ever on a fresh device).
    fun startPairing(passphrase: String) {
        val svc = service
        if (svc != null) svc.startPairing(passphrase) else CameraAgentService.startFirstPairing(context, passphrase)
    }

    // A call always comes first — ringing, connected, or just ended — over
    // Settings, Rename, phrase entry and a pairing attempt alike. Their own
    // state is left alone, so once the call is over the person lands back
    // where they were (apart from text typed so far).
    val callTakesOver = state.activePairingId != null || state.pendingCallOutcome != null
    val screen = if (callTakesOver) null else adminScreen
    when {
        showAdminChoice && !callTakesOver -> {
            AdminChoiceScreen(
                launchOnBoot = config.launchOnBoot,
                onToggleLaunchOnBoot = { enabled ->
                    updateConfig { it.copy(launchOnBoot = enabled) }
                },
                callWakeUp = config.callWakeUp,
                onToggleCallWakeUp = { enabled ->
                    updateConfig { it.copy(callWakeUp = enabled) }
                },
                ringVolume = config.ringVolume,
                onRingVolumeChange = { level ->
                    updateConfig { it.copy(ringVolume = level) }
                },
                onPreviewRingVolume = { level -> service?.previewRingtone(level) },
                onRenameDevice = { adminScreen = AdminScreen.Rename; onAdminChoiceHandled() },
                onOpenConnectionInfo = { adminScreen = AdminScreen.ConnectionInfo; onAdminChoiceHandled() },
                onCancel = onAdminChoiceHandled,
            )
        }
        screen is AdminScreen.ConnectionInfo -> {
            ConnectionInfoScreen(
                service = service,
                state = state,
                onBack = {
                    adminScreen = null
                    onReopenAdminChoice()
                },
            )
        }
        screen is AdminScreen.Rename -> {
            NameEntryScreen(
                initial = config.deviceName,
                isRename = true,
                // Only ever reached via `showAdminChoice` now (see
                // AdminScreen.Rename's own doc) — Back/Save always reopen
                // Device settings, no second context to distinguish.
                onCancel = {
                    adminScreen = null
                    onReopenAdminChoice()
                },
                onDone = { name ->
                    updateConfig { it.copy(deviceName = name) }
                    // Restart so the new name actually gets rebroadcast over
                    // presence now, rather than only on next app launch —
                    // see CameraAgentService.restartAgent's doc.
                    service?.restartAgent()
                    adminScreen = null
                    onReopenAdminChoice()
                },
            )
        }
        screen is AdminScreen.EnteringPhrase -> {
            EnterPhraseScreen(
                onSubmit = { phrase ->
                    startPairing(phrase)
                    adminScreen = AdminScreen.PairingInProgress
                },
                // Falls straight back to the waiting screen (which is where
                // this was always reached from — see AdminScreen.
                // EnteringPhrase's own doc), not into Device settings.
                onCancel = { adminScreen = null },
            )
        }
        screen is AdminScreen.PairingInProgress -> {
            // Both people confirmed and the contact is saved: back to the waiting screen.
            val completed = state.pairingAttempt?.completed == true
            LaunchedEffect(completed) {
                if (completed) {
                    service?.discardPairingAttempt()
                    adminScreen = null
                }
            }
            PairingProgressScreen(
                attempt = state.pairingAttempt,
                service = service,
                onConfirm = { service?.confirmPeer() },
                // The attempt so far is forgotten entirely, same as Cancel
                // below — the next one is a new attempt with its own id and key.
                onRetry = {
                    service?.discardPairingAttempt()
                    adminScreen = AdminScreen.EnteringPhrase
                },
                onCancel = {
                    service?.discardPairingAttempt()
                    adminScreen = null
                },
            )
        }
        !config.hasName -> {
            NameEntryScreen(
                initial = config.deviceName,
                // No prior screen to cancel back to at first launch, but
                // Back still needs to do *something* other than fall
                // through to the system default and exit the app — see
                // NameEntryScreen's own doc.
                onCancel = {},
                onDone = { name ->
                    updateConfig { it.copy(deviceName = name) }
                },
            )
        }
        else -> {
            // Nothing to press — walk up and it's already trying to connect.
            //
            // Only meaningful the very first time this branch is reached
            // with no service bound yet (a cold launch, or just after this
            // device's first-ever pairing is submitted). Gated on
            // `config.isValid` too: with zero contacts there's nothing to
            // connect *to* yet — CameraAgentService refuses to keep running
            // with zero pairings, so calling onConnect here before that
            // would start and immediately self-stop it for nothing. Once
            // `service` is already bound, re-saving this composable's
            // separately-remembered `config` snapshot would silently
            // overwrite whatever CameraAgentService wrote directly in the
            // meantime — confirmed on-device losing a just-confirmed peer
            // pin this way.
            if (service == null && config.isValid) LaunchedEffect(Unit) { onConnect(config) }
            HomeScreen(
                service = service,
                state = state,
                config = config,
                onCyclePreviewPosition = {
                    updateConfig { it.copy(previewCorner = it.previewCorner.next()) }
                },
                onResetPreviewPosition = {
                    // Skipped when already BOTTOM_START — the common case,
                    // since this fires at the start of every call — to avoid
                    // a pointless SharedPreferences write.
                    if (Config.load(context).previewCorner != PreviewCorner.BOTTOM_START) {
                        updateConfig { it.copy(previewCorner = PreviewCorner.BOTTOM_START) }
                    }
                },
                // Mirrors the web client's own #settingsBtn gear.
                onOpenSettings = onReopenAdminChoice,
                // The one and only entry point into "Add contact" now — a
                // trailing row in the contact list itself, not a separate
                // Settings destination (see AdminScreen.EnteringPhrase's doc).
                onAddContact = { adminScreen = AdminScreen.EnteringPhrase },
                // Update this composable's own `config` snapshot directly
                // from the known change, rather than reloading right after —
                // removePairing runs on CameraAgentService's callExecutor
                // asynchronously, so a same-frame reload here could race
                // ahead of the background write and read back the
                // pre-change value. No race possible this way: the outcome
                // is already known. Only `config.isValid` (pairings.
                // isNotEmpty()) actually depends on this; nothing still
                // reads config.pairings for display (see ContactState.
                // autoAnswer's doc for why auto-answer moved off this path).
                onDeleteContact = { id ->
                    service?.removePairing(id)
                    config = config.copy(pairings = config.pairings.filterNot { it.id == id })
                },
                onToggleAutoAnswer = { id, enabled -> service?.setAutoAnswer(id, enabled) },
            )
        }
    }
}

/**
 * Shown once, before this device has any contact — the name rides along on
 * every pairing attempt (see `call-core`'s `own_name`, passed to
 * [CallCoreBridge.startAttempt]) so the other side has
 * something human-readable to show on its own "Pair with [name]?" screen. A
 * plain on-screen-keyboard text field is all this needs: a short name is a
 * handful of keystrokes, not something worth building a QR flow to avoid.
 */
@Composable
private fun NameEntryScreen(
    initial: String,
    isRename: Boolean = false,
    onCancel: (() -> Unit)? = null,
    onDone: (String) -> Unit,
) {
    var name by remember { mutableStateOf(initial) }
    // Always non-null in practice now (both call sites pass a real
    // onCancel — first-launch naming has no prior screen, so its own
    // onCancel is a no-op rather than omitted, to keep Back from falling
    // through to the system default and exiting the app). The null case
    // stays supported rather than making the parameter required, since
    // "no handler at all" and "handler that does nothing" are genuinely
    // different things worth being able to express separately.
    if (onCancel != null) BackHandler(onBack = onCancel)
    // Read at composition time, not inside submit() below — stringResource
    // is @Composable and can't be called from a plain lambda that escapes
    // composition (an event handler invoked later, e.g. from a keyboard
    // callback or a button's onClick).
    val defaultName = stringResource(R.string.nameEntry_defaultNameAndroid)
    // Shared by the button's onClick and the keyboard's Done action below —
    // see CallCoreBridge.sanitizeName's own doc for why this name (which
    // becomes a *peer's* self-reported name from their side the moment it
    // heartbeats out) needs stripping here too, not just on receipt.
    val submit = { onDone(CallCoreBridge.sanitizeName(name.trim()).ifBlank { defaultName }) }
    val titleText = stringResource(if (isRename) R.string.nameEntry_titleRename else R.string.nameEntry_titleNew)
    val textField = @Composable {
        SubmitOnEnterTextField(
            value = name,
            // take(maxNameLength) here too, not just at submit — immediate
            // feedback instead of letting someone type/paste well past the
            // limit. Reads CallCoreBridge.protocolConstants, not a
            // hand-copied literal, same as NostrSignalingClient.kt's own
            // identical cap.
            onValueChange = { name = it.take(CallCoreBridge.protocolConstants.maxNameLength) },
            label = stringResource(R.string.nameEntry_fieldLabel),
            onSubmit = submit,
            modifier = Modifier.fillMaxWidth(),
        )
    }
    val submitButton = @Composable {
        TvButton(onClick = submit) { Text(stringResource(if (isRename) R.string.nameEntry_saveButton else R.string.nameEntry_continueButton)) }
    }
    if (isRename) {
        // Settings-style page destination — upper-left dim title, matching
        // Settings' own (this is reached *from* Settings). Mirrors web's
        // #screenRename, whose own comment says the same thing.
        PageScreen(title = titleText) {
            Text(
                stringResource(R.string.nameEntry_subtitle),
                color = GeneratedColor.colorTextDim,
                style = MaterialTheme.typography.bodyMedium,
                textAlign = TextAlign.Center,
                modifier = Modifier.fillMaxWidth(),
            )
            textField()
            submitButton()
        }
    } else {
        // First-time naming — a real centered dialog, the same shape as
        // OutcomeScreen/CallOutcomeScreen (bright title, this is a genuine
        // moment, not a page to manage something). Mirrors web's
        // #screenName, a plain .panel with a bright .panel h1, distinct
        // from #screenRename above.
        Box(modifier = Modifier.fillMaxSize().porchlightScreenBackground(), contentAlignment = Alignment.Center) {
            Column(
                horizontalAlignment = Alignment.CenterHorizontally,
                verticalArrangement = Arrangement.spacedBy(Dimens.spacingPanelContentGap),
                modifier = Modifier.widthIn(max = Dimens.sizeMaxContentWidth).padding(horizontal = Dimens.spacingScreenPadding),
            ) {
                Text(titleText, color = GeneratedColor.colorTextPrimary, style = MaterialTheme.typography.headlineSmall, textAlign = TextAlign.Center)
                Text(
                    stringResource(R.string.nameEntry_subtitle),
                    color = GeneratedColor.colorTextDim,
                    style = MaterialTheme.typography.bodyMedium,
                    textAlign = TextAlign.Center,
                )
                textField()
                submitButton()
            }
        }
    }
}

/**
 * Reached via the on-screen settings gear (see WaitingScreen's own doc).
 * Down to two things now: "Rename this device", and the launch-on-boot
 * toggle (see Config.launchOnBoot's own doc for why this is opt-in, not
 * automatic). "Add contact" and per-contact management (Delete,
 * auto-answer) both moved onto WaitingScreen's own contact list directly,
 * so there's no "Contacts" destination left here to route to. There's no
 * whole-device "I think this was compromised" action either — every
 * `Pairing` is its own independent identity, so recovering from a
 * suspected compromise is just Delete + Add contact for the one contact
 * that's actually affected.
 */
@Composable
private fun AdminChoiceScreen(
    launchOnBoot: Boolean,
    onToggleLaunchOnBoot: (Boolean) -> Unit,
    callWakeUp: Boolean,
    onToggleCallWakeUp: (Boolean) -> Unit,
    ringVolume: Config.RingVolume,
    onRingVolumeChange: (Config.RingVolume) -> Unit,
    onPreviewRingVolume: (Config.RingVolume) -> Unit,
    onRenameDevice: () -> Unit,
    onOpenConnectionInfo: () -> Unit,
    onCancel: () -> Unit,
) {
    BackHandler(onBack = onCancel)
    val focusRequester = remember { FocusRequester() }
    RequestFocusOnMount(focusRequester)

    val context = LocalContext.current
    // OkHttp's own callback thread, not callExecutor — UpdateChecker's
    // own doc is explicit that it never touches callExecutor, so
    // hopping to the main thread has to happen here, at the UI-state
    // boundary, the same way CameraAgentService's mainHandler does it.
    val mainHandler = remember { Handler(Looper.getMainLooper()) }
    var checkResult by remember { mutableStateOf<UpdateCheckResult?>(null) }
    // Checks the moment Settings opens, no button to tap first — force
    // = true so this always gives a real answer, even for a release
    // the silent scheduled check already found and notified about once
    // (see UpdateChecker.checkNow's own doc). Keyed on Unit: Settings
    // is a fresh composition every time it's opened (see AppRoot's own
    // reload-on-open comment above), so this naturally re-checks each
    // visit without a separate trigger.
    LaunchedEffect(Unit) {
        UpdateChecker.checkNow(context, force = true) { result ->
            mainHandler.post { checkResult = result }
        }
    }
    val message = when (val result = checkResult) {
        null -> stringResource(R.string.settings_update_checking)
        UpdateCheckResult.Disabled -> stringResource(R.string.settings_update_disabled)
        UpdateCheckResult.UpToDate -> stringResource(R.string.settings_update_upToDate, BuildConfig.VERSION_NAME)
        is UpdateCheckResult.Downloading -> stringResource(R.string.settings_update_downloading, result.versionName)
        is UpdateCheckResult.Ready -> stringResource(R.string.settings_update_ready, BuildConfig.VERSION_NAME, result.versionName)
        is UpdateCheckResult.Failed -> stringResource(R.string.settings_update_failed, result.reason)
    }

    // A 2-column, 3-row table, centered in PageScreen's own fixed-max-width
    // block — label left, control right, each row's own fillMaxWidth()
    // stretching it to that shared width so the right-column controls
    // (arrow button / switch / Install) land on a consistent right edge
    // despite each row's own content differing.
    PageScreen(title = stringResource(R.string.settings_title)) {
                // Each row's own left-column label stays muted until the
                // right-column control it describes actually has focus —
                // same "label brightens with its control's own focus"
                // pattern WaitingScreen's Auto-answer switch already uses.
                val ringVolumeInteractionSource = remember { MutableInteractionSource() }
                val ringVolumeFocused by ringVolumeInteractionSource.collectIsFocusedAsState()
                // Plays the chime at the chosen level only once the person
                // has stopped cycling — each press restarts this delay, so
                // stepping through Off/Low/Medium/High doesn't blast every
                // level on the way past. Skipped on first composition: just
                // opening Settings shouldn't ring.
                var ringVolumeTouched by remember { mutableStateOf(false) }
                val ringVolumeLabel = "${ringVolume.percent}%"
                LaunchedEffect(ringVolume) {
                    if (!ringVolumeTouched) return@LaunchedEffect
                    delay(RING_VOLUME_PREVIEW_DELAY_MS)
                    onPreviewRingVolume(ringVolume)
                }
                Row(
                    modifier = Modifier.fillMaxWidth(),
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(Dimens.spacingContactRowGap),
                ) {
                    Text(
                        stringResource(R.string.settings_ringVolume_title),
                        color = if (ringVolumeFocused) GeneratedColor.colorTextPrimary else GeneratedColor.colorTextDim,
                        modifier = Modifier.weight(1f),
                    )
                    // Click cycles forward (wrapping); Left/Right step
                    // without wrapping, so overshooting Off doesn't jump to
                    // High.
                    FocusableStepSlider(
                        steps = Config.RingVolume.entries.size,
                        selected = ringVolume.ordinal,
                        onSelectedChange = { ringVolumeTouched = true; onRingVolumeChange(Config.RingVolume.entries[it]) },
                        interactionSource = ringVolumeInteractionSource,
                        modifier = Modifier.focusRequester(focusRequester).semantics { stateDescription = ringVolumeLabel }.onPreviewKeyEvent { event ->
                            if (event.type != KeyEventType.KeyDown) return@onPreviewKeyEvent false
                            when (event.key) {
                                Key.DirectionRight -> {
                                    if (ringVolume != Config.RingVolume.P100) { ringVolumeTouched = true; onRingVolumeChange(ringVolume.next()) }
                                    true
                                }
                                Key.DirectionLeft -> {
                                    if (ringVolume != Config.RingVolume.P0) { ringVolumeTouched = true; onRingVolumeChange(ringVolume.previous()) }
                                    true
                                }
                                else -> false
                            }
                        },
                    )
                }
                val renameInteractionSource = remember { MutableInteractionSource() }
                val renameFocused by renameInteractionSource.collectIsFocusedAsState()
                Row(
                    modifier = Modifier.fillMaxWidth(),
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(Dimens.spacingContactRowGap),
                ) {
                    Text(
                        stringResource(R.string.settings_renameRow),
                        color = if (renameFocused) GeneratedColor.colorTextPrimary else GeneratedColor.colorTextDim,
                        modifier = Modifier.weight(1f),
                    )
                    TvButton(
                        onClick = onRenameDevice,
                        interactionSource = renameInteractionSource,
                    ) { Icon(Icons.Filled.ArrowForward, contentDescription = stringResource(R.string.settings_renameRow), modifier = Modifier.size(Dimens.dimension20)) }
                }
                val launchOnBootInteractionSource = remember { MutableInteractionSource() }
                val launchOnBootFocused by launchOnBootInteractionSource.collectIsFocusedAsState()
                Row(
                    modifier = Modifier.fillMaxWidth(),
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(Dimens.spacingContactRowGap),
                ) {
                    Column(modifier = Modifier.weight(1f)) {
                        Text(
                            stringResource(R.string.settings_kioskMode_title),
                            color = if (launchOnBootFocused) GeneratedColor.colorTextPrimary else GeneratedColor.colorTextDim,
                        )
                        Text(
                            stringResource(R.string.settings_kioskMode_subtitle),
                            color = GeneratedColor.colorTextDim,
                            style = MaterialTheme.typography.bodySmall,
                        )
                    }
                    FocusableSwitch(
                        checked = launchOnBoot,
                        onCheckedChange = onToggleLaunchOnBoot,
                        interactionSource = launchOnBootInteractionSource,
                    )
                }
                // Switching it on at all is a one-time adb step (see the
                // README; the Portal's Settings has no screen for it), so
                // until the service is enabled this is off and can't be
                // reached — same idea as the update line's Install button.
                val callWakeUpPossible by CallWakeUpAccessibilityService.enabled.collectAsState()
                val callWakeUpInteractionSource = remember { MutableInteractionSource() }
                val callWakeUpFocused by callWakeUpInteractionSource.collectIsFocusedAsState()
                Row(
                    modifier = Modifier.fillMaxWidth(),
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(Dimens.spacingContactRowGap),
                ) {
                    Column(modifier = Modifier.weight(1f)) {
                        Text(
                            stringResource(R.string.settings_callWakeUp_title),
                            color = if (callWakeUpFocused) GeneratedColor.colorTextPrimary else GeneratedColor.colorTextDim,
                        )
                        Text(
                            stringResource(if (callWakeUpPossible) R.string.settings_callWakeUp_subtitle else R.string.settings_callWakeUp_needsSetup),
                            color = GeneratedColor.colorTextDim,
                            style = MaterialTheme.typography.bodySmall,
                        )
                    }
                    FocusableSwitch(
                        checked = callWakeUpPossible && callWakeUp,
                        onCheckedChange = onToggleCallWakeUp,
                        interactionSource = callWakeUpInteractionSource,
                        modifier = Modifier
                            .alpha(if (callWakeUpPossible) 1f else GeneratedOpacity.opacity50)
                            .focusProperties { canFocus = callWakeUpPossible },
                    )
                }
                val connectionInfoInteractionSource = remember { MutableInteractionSource() }
                val connectionInfoFocused by connectionInfoInteractionSource.collectIsFocusedAsState()
                Row(
                    modifier = Modifier.fillMaxWidth(),
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(Dimens.spacingContactRowGap),
                ) {
                    Text(
                        stringResource(R.string.settings_connectionInfoRow),
                        color = if (connectionInfoFocused) GeneratedColor.colorTextPrimary else GeneratedColor.colorTextDim,
                        modifier = Modifier.weight(1f),
                    )
                    TvButton(
                        onClick = onOpenConnectionInfo,
                        interactionSource = connectionInfoInteractionSource,
                    ) { Icon(Icons.Filled.ArrowForward, contentDescription = stringResource(R.string.settings_connectionInfoRow), modifier = Modifier.size(Dimens.dimension20)) }
                }
                val installInteractionSource = remember { MutableInteractionSource() }
                val installFocused by installInteractionSource.collectIsFocusedAsState()
                val selfInstallPossible = remember { UpdateChecker.canSelfInstall(context) }
                val updateReady = checkResult is UpdateCheckResult.Ready
                val canInstall = updateReady && selfInstallPossible
                Row(
                    modifier = Modifier.fillMaxWidth(),
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(Dimens.spacingContactRowGap),
                ) {
                    // A Box, not just the Text directly: message starts as
                    // "Checking for updates…" and almost immediately swaps
                    // to a differently-sized final string once the check
                    // resolves. Harmless now that the table's own width is
                    // PageScreen's fixed max-width rather than computed from
                    // content, but kept anyway (the invisible probes below
                    // cost nothing and still guard the Install button's own
                    // left edge from any future layout that isn't fixed-width).
                    Column(modifier = Modifier.weight(1f)) {
                        Box {
                            Text(stringResource(R.string.settings_update_checking), modifier = Modifier.alpha(0f))
                            Text(stringResource(R.string.settings_update_disabled), modifier = Modifier.alpha(0f))
                            Text(stringResource(R.string.settings_update_upToDate, BuildConfig.VERSION_NAME), modifier = Modifier.alpha(0f))
                            Text(stringResource(R.string.settings_update_downloading, BuildConfig.VERSION_NAME), modifier = Modifier.alpha(0f))
                            Text(stringResource(R.string.settings_update_ready, BuildConfig.VERSION_NAME, BuildConfig.VERSION_NAME), modifier = Modifier.alpha(0f))
                            Text(stringResource(R.string.settings_update_failed, "server (500)"), modifier = Modifier.alpha(0f))
                            Text(
                                message,
                                color = if (installFocused) GeneratedColor.colorTextPrimary else GeneratedColor.colorTextDim,
                            )
                        }
                        // An update is there but installing it from here can't
                        // work yet (see UpdateChecker.canSelfInstall).
                        if (updateReady && !selfInstallPossible) {
                            Text(
                                stringResource(R.string.settings_update_installNeedsSetup),
                                color = GeneratedColor.colorTextDim,
                                style = MaterialTheme.typography.bodySmall,
                            )
                        }
                    }
                    // Only enabled once there's actually something to
                    // install and installing can work — always present, so
                    // the table's right column stays put rather than the
                    // row reflowing.
                    // Not just greyed out: a disabled button can still take
                    // D-pad focus (see TvButton), and there's nothing here
                    // to land on until an install can actually succeed.
                    TvButton(
                        enabled = canInstall,
                        onClick = { UpdateChecker.installOrRequestPermission(context) },
                        interactionSource = installInteractionSource,
                        modifier = Modifier.focusProperties { canFocus = canInstall },
                    ) { Text(stringResource(R.string.settings_update_installButton)) }
                }
    }
}
