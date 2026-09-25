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
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.IntrinsicSize
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
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
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.input.key.Key
import androidx.compose.ui.input.key.KeyEventType
import androidx.compose.ui.input.key.key
import androidx.compose.ui.input.key.onPreviewKeyEvent
import androidx.compose.ui.input.key.type
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.input.ImeAction
import androidx.tv.material3.Border
import androidx.tv.material3.ClickableSurfaceDefaults
import androidx.tv.material3.ClickableSurfaceScale
import androidx.tv.material3.Icon
import androidx.tv.material3.MaterialTheme
import androidx.tv.material3.ProvideTextStyle
import androidx.tv.material3.Switch
import androidx.tv.material3.SwitchDefaults
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
        service?.hangUp()
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

    TvSurface(
        onClick = onClick,
        enabled = enabled,
        modifier = modifier,
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
            border = Border(border = BorderStroke(Dimens.borderWidthDefault, hue.copy(alpha = borderAlpha)), shape = TvButtonShape),
            focusedBorder = Border(border = BorderStroke(Dimens.borderWidthDefault, hue.copy(alpha = focusedBorderAlpha)), shape = TvButtonShape),
        ),
        colors = ClickableSurfaceDefaults.colors(
            containerColor = hue.copy(alpha = containerAlpha),
            contentColor = contentColor,
            focusedContainerColor = hue.copy(alpha = focusedContainerAlpha),
            focusedContentColor = contentColor,
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
        ProvideTextStyle(MaterialTheme.typography.labelLarge.copy(fontWeight = FontWeight(GeneratedType.fontWeightRegular))) {
            Row(
                horizontalArrangement = Arrangement.Center,
                verticalAlignment = Alignment.CenterVertically,
                modifier = Modifier.padding(
                    horizontal = Dimens.buttonPaddingBaseHorizontal,
                    vertical = Dimens.buttonPaddingBaseVertical,
                ),
                content = content,
            )
        }
    }
}

/**
 * A tv.material3 [Switch] with a focus effect layered on top — its own
 * [SwitchColors] has no focused-state slots at all (checked/unchecked ×
 * enabled/disabled only, confirmed against the library's actual API), so
 * without this a Switch is the one control in the app that gives no visual
 * feedback when it has D-pad focus, unlike [TvButton]'s own border/fill
 * brightening. Unchecked reuses [TvButton]'s Neutral tint formula exactly
 * — the same [GeneratedOpacity] alpha tokens over white, unfocused vs
 * focused, so it reads as the same family of control as every neutral
 * button in the app. Checked (an already-"on" control) instead always
 * sits at the *focused* accent alpha level as its baseline — plain
 * unfocused-accent read as too faint for something already toggled on —
 * and on focus, only the track's alpha rises further to match the
 * border's exactly, so the whole switch reads as one solid block of the
 * same green the in-call Accept/Call buttons use, rather than restating
 * the border/fill split the unfocused state already has. The thumb stays
 * a plain, focus-independent on/off indicator, same as a TvButton's own
 * content color never changing with focus either.
 */
@Composable
internal fun FocusableSwitch(
    checked: Boolean,
    onCheckedChange: (Boolean) -> Unit,
    modifier: Modifier = Modifier,
    interactionSource: MutableInteractionSource = remember { MutableInteractionSource() },
) {
    val focused by interactionSource.collectIsFocusedAsState()
    val neutralContainerAlpha = if (focused) GeneratedOpacity.buttonTintNeutralContainerFocused else GeneratedOpacity.buttonTintNeutralContainer
    val neutralBorderAlpha = if (focused) GeneratedOpacity.buttonTintNeutralBorderFocused else GeneratedOpacity.buttonTintNeutralBorder
    val checkedBorderAlpha = GeneratedOpacity.buttonTintAccentBorderFocused
    val checkedTrackAlpha = if (focused) GeneratedOpacity.buttonTintAccentBorderFocused else GeneratedOpacity.buttonTintAccentContainerFocused
    Switch(
        checked = checked,
        onCheckedChange = onCheckedChange,
        colors = SwitchDefaults.colors(
            checkedThumbColor = GeneratedColor.colorTextPrimary,
            checkedTrackColor = GeneratedColor.colorStatusOk.copy(alpha = checkedTrackAlpha),
            checkedBorderColor = GeneratedColor.colorStatusOk.copy(alpha = checkedBorderAlpha),
            uncheckedThumbColor = GeneratedColor.colorTextDim,
            uncheckedTrackColor = Color.White.copy(alpha = neutralContainerAlpha),
            uncheckedBorderColor = Color.White.copy(alpha = neutralBorderAlpha),
        ),
        interactionSource = interactionSource,
        modifier = modifier,
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
    // "Add contact" only — there's no "Reconnect": a stale contact is just
    // Delete + Add contact again, so this carries no pairing id at all;
    // startPairing always mints a brand-new one. Reached directly from a
    // trailing row in WaitingScreen's own contact list (both once this
    // device already has contacts, and pre-onboarding with zero — the
    // waiting screen renders either way, empty list and all). Per-contact
    // management (Delete, auto-answer) lives right on each ContactRow now,
    // not behind a separate Contacts screen. Exactly one entry point means
    // Back always falls straight back to the waiting screen.
    data object EnteringPhrase : AdminScreen
    // A phrase was submitted, minting [pairingId] — waiting on the SPAKE2
    // exchange to resolve (candidate found, collision, or timeout). See
    // PairingProgressScreen's doc. Always a brand-new, never-yet-confirmed
    // contact now, so cancelling (or abandoning via a further retry) always
    // means fully forgetting it (CameraAgentService.removePairing) rather
    // than leaving a permanent "Unnamed contact" stub behind.
    data class PairingInProgress(val pairingId: String) : AdminScreen
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
    val state by (service?.state?.collectAsState() ?: remember { mutableStateOf(CameraAgentService.AgentState()) })
    var adminScreen by remember { mutableStateOf<AdminScreen?>(null) }

    // Keep the screen on while a call is active so it can't sleep mid-call.
    // Keyed on activePairingId (same signal HomeScreens.kt's own
    // activeContact uses), not state.running — found live on real Portal
    // hardware that running stays true the entire time the background
    // service is connected to signaling, not just during a call, which
    // kept the screen (and Immortal's screensaver) from ever going idle
    // while just sitting on the waiting screen.
    LaunchedEffect(state.activePairingId) {
        val flag = WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON
        (context as? android.app.Activity)?.window?.let {
            if (state.activePairingId != null) it.addFlags(flag) else it.clearFlags(flag)
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

    // Constructs a fresh Pairing directly when the service isn't running
    // yet — the pre-onboarding case (this device just finished "Initial
    // setup" but has zero contacts yet, so CameraAgentService hasn't
    // started: it only starts once Config.isValid). See
    // CameraAgentService.startFirstPairing's doc for why persisting it and
    // stashing the passphrase has to happen here.
    fun startPairing(passphrase: String): Pairing {
        val svc = service
        if (svc != null) return svc.startPairing(passphrase)
        val fresh = Pairing(id = UUID.randomUUID().toString(), ownPrivateKeyHex = KeyPair().privKey!!.toHexKey())
        CameraAgentService.startFirstPairing(context, fresh, passphrase)
        return fresh
    }

    val screen = adminScreen
    when {
        showAdminChoice -> {
            AdminChoiceScreen(
                launchOnBoot = config.launchOnBoot,
                onToggleLaunchOnBoot = { enabled ->
                    // Same reasoning as onCyclePreviewPosition below —
                    // reload fresh so this doesn't stomp a pairing change
                    // CameraAgentService made directly since `config` was
                    // last refreshed in this composable.
                    val c = Config.load(context).copy(launchOnBoot = enabled)
                    Config.save(context, c)
                    config = c
                },
                onRenameDevice = { adminScreen = AdminScreen.Rename; onAdminChoiceHandled() },
                onCancel = onAdminChoiceHandled,
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
                    // Reload fresh rather than mutate this composable's own
                    // (possibly stale) `config` snapshot — CameraAgentService
                    // can have pinned a peer directly to disk since `config`
                    // was last refreshed here, so saving a stale copy would
                    // silently discard that pin.
                    val c = Config.load(context).copy(deviceName = name)
                    Config.save(context, c)
                    config = c
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
                    val pairing = startPairing(phrase)
                    config = Config.load(context)
                    adminScreen = AdminScreen.PairingInProgress(pairing.id)
                },
                // Falls straight back to the waiting screen (which is where
                // this was always reached from — see AdminScreen.
                // EnteringPhrase's own doc), not into Device settings.
                onCancel = { adminScreen = null },
            )
        }
        screen is AdminScreen.PairingInProgress -> {
            PairingProgressScreen(
                pairingId = screen.pairingId,
                contacts = state.contacts,
                onConfirm = { id, publicKeyHex -> service?.confirmPeer(id, publicKeyHex); adminScreen = null },
                // The attempt so far (including its unconfirmed stub) is
                // forgotten entirely, same as Cancel below — startPairing
                // mints an unrelated fresh id, so there's nothing to reuse
                // this one for.
                onRetry = {
                    service?.removePairing(screen.pairingId)
                    adminScreen = AdminScreen.EnteringPhrase
                },
                onCancel = {
                    service?.removePairing(screen.pairingId)
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
                    val c = config.copy(deviceName = name)
                    Config.save(context, c)
                    config = c
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
                    // Same reasoning as AdminScreen.Rename's onDone above —
                    // reload fresh so this doesn't stomp a pairing change
                    // CameraAgentService made directly since `config` was
                    // last refreshed in this composable.
                    val c = Config.load(context).copy(previewCorner = config.previewCorner.next())
                    Config.save(context, c)
                    config = c
                },
                onResetPreviewPosition = {
                    // Same reload-fresh reasoning as onCyclePreviewPosition
                    // above. A no-op write (skipped, not just harmless) when
                    // already BOTTOM_START — the common case, since this
                    // fires at the start of every call — avoids a pointless
                    // SharedPreferences write most of the time.
                    val fresh = Config.load(context)
                    if (fresh.previewCorner != PreviewCorner.BOTTOM_START) {
                        val c = fresh.copy(previewCorner = PreviewCorner.BOTTOM_START)
                        Config.save(context, c)
                        config = c
                    }
                },
                // Mirrors the web client's own #settingsBtn gear.
                onOpenSettings = onReopenAdminChoice,
                // The one and only entry point into "Add contact" now — a
                // trailing row in the contact list itself, not a separate
                // Settings destination (see AdminScreen.EnteringPhrase's doc).
                onAddContact = { adminScreen = AdminScreen.EnteringPhrase },
                // Update this composable's own `config` snapshot directly
                // from the known change, rather than reloading right after
                // — removePairing/setAutoAnswer both run on
                // CameraAgentService's callExecutor asynchronously, so a
                // same-frame reload here reliably raced ahead of the
                // background write and read back the pre-change value (for
                // setAutoAnswer specifically, the switch visibly showed the
                // *previous* state after every toggle). No race possible
                // this way: the outcome is already known.
                onDeleteContact = { id ->
                    service?.removePairing(id)
                    config = config.copy(pairings = config.pairings.filterNot { it.id == id })
                },
                onToggleAutoAnswer = { id, enabled ->
                    service?.setAutoAnswer(id, enabled)
                    config = config.copy(pairings = config.pairings.map { if (it.id == id) it.copy(autoAnswer = enabled) else it })
                },
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
    // Shared by the button's onClick and the keyboard's Done action below —
    // see CallCoreBridge.sanitizeName's own doc for why this name (which
    // becomes a *peer's* self-reported name from their side the moment it
    // heartbeats out) needs stripping here too, not just on receipt.
    val submit = { onDone(CallCoreBridge.sanitizeName(name.trim()).ifBlank { "Device" }) }
    Column(modifier = Modifier.fillMaxSize().padding(Dimens.spacingScreenPadding)) {
        // Dimmed like Settings' own title — every screen's title uses this
        // exact color/style now, not just Settings.
        Text(
            if (isRename) "Rename this device" else "Name this device",
            color = GeneratedColor.colorTextDim,
            style = MaterialTheme.typography.headlineSmall,
        )
        // Everything below the title centered as a block, same
        // width(IntrinsicSize.Max)-in-a-centered-Box pattern Settings uses.
        Box(modifier = Modifier.weight(1f).fillMaxWidth(), contentAlignment = Alignment.Center) {
            Column(
                modifier = Modifier.width(IntrinsicSize.Max),
                verticalArrangement = Arrangement.spacedBy(Dimens.spacingPanelContentGap),
            ) {
                Text(
                    "Shown to other devices during pairing and in their contacts list.",
                    color = GeneratedColor.colorTextDim,
                    style = MaterialTheme.typography.bodyMedium,
                )
                OutlinedTextField(
                    value = name,
                    // take(maxNameLength) here too, not just at submit — immediate
                    // feedback instead of letting someone type/paste well past the
                    // limit. Reads CallCoreBridge.protocolConstants, not a
                    // hand-copied literal, same as NostrSignalingClient.kt's own
                    // identical cap.
                    onValueChange = { name = it.take(CallCoreBridge.protocolConstants.maxNameLength) },
                    label = { M3Text("Name") },
                    modifier = Modifier.fillMaxWidth().onPreviewKeyEvent { event ->
                        // See HardwareEnterKeyUpGuard's own doc for the full
                        // mechanism and why arm() is needed here too, not just
                        // submit(): consuming this KeyDown does *not* stop this
                        // same physical press's KeyUp from being separately,
                        // independently dispatched a moment later, straight to
                        // whatever the *next* screen's first focusable turns out
                        // to be.
                        if (event.type == KeyEventType.KeyDown && (event.key == Key.Enter || event.key == Key.NumPadEnter)) {
                            submit()
                            HardwareEnterKeyUpGuard.arm()
                            true
                        } else {
                            false
                        }
                    },
                    // singleLine forces the IME to offer a Done action instead of a
                    // newline key for Enter/Return — without it, Enter just inserts
                    // "\n" into a name, with no way to submit from the keyboard.
                    singleLine = true,
                    keyboardOptions = KeyboardOptions(imeAction = ImeAction.Done),
                    keyboardActions = KeyboardActions(onDone = { submit() }),
                )
                TvButton(onClick = submit) { Text(if (isRename) "Save" else "Continue") }
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
    onRenameDevice: () -> Unit,
    onCancel: () -> Unit,
) {
    BackHandler(onBack = onCancel)
    val focusRequester = remember { FocusRequester() }
    LaunchedEffect(Unit) { focusRequester.requestFocus() }

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
    val versionSuffix = " (v${BuildConfig.VERSION_NAME})"
    val message = when (val result = checkResult) {
        null -> "Checking for updates…"
        UpdateCheckResult.Disabled -> "Update checking isn't set up for this build."
        UpdateCheckResult.UpToDate -> "You're on the latest version$versionSuffix"
        is UpdateCheckResult.Downloading -> "Downloading ${result.versionName}…"
        is UpdateCheckResult.Ready -> "${result.versionName} downloaded."
        is UpdateCheckResult.Failed -> "Couldn't check for updates (${result.reason})."
    }

    Column(modifier = Modifier.fillMaxSize().porchlightScreenBackground().padding(Dimens.spacingScreenPadding)) {
        // Dimmed like a plain navigational label (matches WaitingScreen's
        // own title treatment elsewhere), not the app's brightest text —
        // this is "which screen am I on," not content the user actually
        // came here to read.
        Text("Settings", color = GeneratedColor.colorTextDim, style = MaterialTheme.typography.headlineSmall)

        // A 2-column, 3-row table, centered in whatever space is left below
        // the title — label left, control right. width(IntrinsicSize.Max)
        // + fillMaxWidth() per row is the same "every row shares the widest
        // row's width" trick WaitingScreen's own contact rows already use
        // (see that file's own doc), so the right-column controls (arrow
        // button / switch / Install) land on a consistent right edge
        // despite each row's own content differing.
        Box(modifier = Modifier.weight(1f).fillMaxWidth(), contentAlignment = Alignment.Center) {
            Column(
                modifier = Modifier.width(IntrinsicSize.Max),
                verticalArrangement = Arrangement.spacedBy(Dimens.spacingPanelContentGap),
            ) {
                // Each row's own left-column label stays muted until the
                // right-column control it describes actually has focus —
                // same "label brightens with its control's own focus"
                // pattern WaitingScreen's Auto-answer switch already uses.
                val renameInteractionSource = remember { MutableInteractionSource() }
                val renameFocused by renameInteractionSource.collectIsFocusedAsState()
                Row(
                    modifier = Modifier.fillMaxWidth(),
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(Dimens.spacingContactRowGap),
                ) {
                    Text(
                        "Rename this device",
                        color = if (renameFocused) GeneratedColor.colorTextPrimary else GeneratedColor.colorTextDim,
                        modifier = Modifier.weight(1f),
                    )
                    TvButton(
                        onClick = onRenameDevice,
                        interactionSource = renameInteractionSource,
                        modifier = Modifier.focusRequester(focusRequester),
                    ) { Icon(Icons.Filled.ArrowForward, contentDescription = "Rename this device", modifier = Modifier.size(Dimens.dimension20)) }
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
                            "Kiosk mode",
                            color = if (launchOnBootFocused) GeneratedColor.colorTextPrimary else GeneratedColor.colorTextDim,
                        )
                        Text(
                            "Bring Porchlight up automatically after (re)boot and when the screensaver ends",
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
                val installInteractionSource = remember { MutableInteractionSource() }
                val installFocused by installInteractionSource.collectIsFocusedAsState()
                Row(
                    modifier = Modifier.fillMaxWidth(),
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(Dimens.spacingContactRowGap),
                ) {
                    // A Box, not just the Text directly: message starts as
                    // "Checking for updates…" and almost immediately swaps
                    // to a differently-sized final string once the check
                    // resolves, which — through the outer Column's shared
                    // width(IntrinsicSize.Max) — visibly shifted every row
                    // in the table by that width difference. The invisible
                    // probes below cover every message length this row can
                    // actually show, so the row (and the whole table) is
                    // sized once, up front, for the widest of them, and
                    // never changes size again as checkResult resolves.
                    Box(modifier = Modifier.weight(1f)) {
                        Text("Checking for updates…", modifier = Modifier.alpha(0f))
                        Text("Update checking isn't set up for this build.", modifier = Modifier.alpha(0f))
                        Text("You're on the latest version$versionSuffix", modifier = Modifier.alpha(0f))
                        Text("Downloading v${BuildConfig.VERSION_NAME}…", modifier = Modifier.alpha(0f))
                        Text("v${BuildConfig.VERSION_NAME} downloaded.", modifier = Modifier.alpha(0f))
                        Text("Couldn't check for updates (server (500)).", modifier = Modifier.alpha(0f))
                        Text(
                            message,
                            color = if (installFocused) GeneratedColor.colorTextPrimary else GeneratedColor.colorTextDim,
                        )
                    }
                    // Only enabled once there's actually something to
                    // install — always present, so the table's right
                    // column stays put rather than the row reflowing.
                    TvButton(
                        enabled = checkResult is UpdateCheckResult.Ready,
                        onClick = { UpdateChecker.installOrRequestPermission(context) },
                        interactionSource = installInteractionSource,
                    ) { Text("Install") }
                }
            }
        }
    }
}
