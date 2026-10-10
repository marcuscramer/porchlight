package dev.porchlight.app

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.tv.material3.MaterialTheme
import androidx.tv.material3.Text
import dev.porchlight.app.ui.theme.Dimens
import dev.porchlight.app.ui.theme.GeneratedColor
import kotlinx.coroutines.delay

// ---------------------------------------------------------------------------
// Passphrase-based pairing UI — one screen for adding a contact, full stop.
// There's no generator/enterer, show/scan, or A/B distinction: both people
// agree on a phrase together (verbally, during a call) and each just types
// it into their own device.
// ---------------------------------------------------------------------------

/**
 * Holds the screen on while a pairing screen is showing. The Portal's screen-off timeout is a minute, and without
 * this its screensaver covers a pairing that is still waiting for the other person (the attempt keeps counting down).
 */
@Composable
private fun KeepScreenOn() {
    val view = LocalView.current
    DisposableEffect(view) {
        view.keepScreenOn = true
        onDispose { view.keepScreenOn = false }
    }
}

/**
 * The one and only pairing entry screen, for a brand-new contact. Nothing to
 * generate or show/read here, so nothing for either side to be "first" at:
 * both people agree on a phrase together (verbally, during a call) and each
 * just types it into their own device.
 */
@Composable
fun EnterPhraseScreen(onSubmit: (String) -> Unit, onCancel: () -> Unit) {
    BackHandler(onBack = onCancel)
    KeepScreenOn()
    var phrase by remember { mutableStateOf("") }
    val focusRequester = remember { FocusRequester() }
    RequestFocusOnMount(focusRequester)
    // Shared by the button's onClick and the keyboard's Done action below.
    val submit = { if (phrase.isNotBlank()) onSubmit(phrase) }
    // Same dim-title/centered-content shape every other settings-gear-
    // reachable page screen uses (PageScreen's own doc, MainActivity.kt).
    PageScreen(title = stringResource(R.string.pairing_addContactTitle), onBack = onCancel) {
        Text(
            stringResource(R.string.pairing_phraseInstructions),
            color = GeneratedColor.colorTextDim,
            style = MaterialTheme.typography.bodyMedium,
            textAlign = TextAlign.Center,
            modifier = Modifier.fillMaxWidth(),
        )
        Text(
            stringResource(R.string.pairing_oneTimeNote),
            color = GeneratedColor.colorTextDim,
            style = MaterialTheme.typography.bodyMedium,
            textAlign = TextAlign.Center,
            modifier = Modifier.fillMaxWidth(),
        )
        SubmitOnEnterTextField(
            value = phrase,
            // take(200): generous for "a short phrase," but a real bound —
            // this becomes a passphrase string fed to CallCoreBridge/JNI,
            // and while that path is panic-safe regardless, there's no
            // reason to let an accidental massive paste through.
            onValueChange = { phrase = it.take(GeneratedSharedConfig.MAX_PHRASE_LENGTH) },
            label = stringResource(R.string.pairing_phraseFieldLabel),
            onSubmit = submit,
            modifier = Modifier.fillMaxWidth().focusRequester(focusRequester),
        )
        TvButton(onClick = submit, enabled = phrase.isNotBlank()) {
            Text(stringResource(R.string.pairing_startPairingButton))
        }
    }
}

/**
 * Shown from the moment a phrase is submitted until the attempt resolves
 * one of three ways: a candidate appears (crypto-confirmed, just needs the
 * final human tap — see [NameConfirmScreen]), a collision is detected, or
 * the live window times out with nobody found. Shows [attempt] (null while
 * the attempt is still being started) rather than owning any state of its
 * own — the same "observe CameraAgentService.state, don't duplicate it"
 * pattern every other screen in this app already uses.
 */
@Composable
fun PairingProgressScreen(
    attempt: CameraAgentService.PairingAttemptState?,
    service: CameraAgentService?,
    onConfirm: () -> Unit,
    onRetry: () -> Unit,
    onCancel: () -> Unit,
) {
    KeepScreenOn()
    val candidate = attempt?.candidate
    when {
        attempt?.notAccepted == true -> {
            LaunchedEffect(Unit) {
                delay(AUTO_DISMISS_DELAY_MS)
                onCancel()
            }
            OutcomeScreen(
                title = stringResource(R.string.pairing_notConfirmedTitle),
                message = stringResource(R.string.pairing_notConfirmedMessage),
                actionLabel = stringResource(R.string.pairing_tryAgainButton),
                onAction = onRetry,
                onCancel = onCancel,
                textLines = PAIRING_TEXT_LINES,
            )
        }
        candidate != null && attempt.accepted -> WaitingForDeviceScreen(attempt.id, service, onCancel, waitingForName = candidate.name)
        candidate != null -> NameConfirmScreen(
            candidate = candidate,
            onConfirm = onConfirm,
            onCancel = onCancel,
        )
        attempt?.collision == true -> {
            // Auto-dismiss to onCancel only, never onAction — safe here
            // because giving up on the attempt is the passive outcome,
            // unlike this same OutcomeScreen composable's other caller
            // (WaitingScreen's delete-confirmation), which does NOT get
            // this treatment since its onAction is destructive.
            LaunchedEffect(Unit) {
                delay(AUTO_DISMISS_DELAY_MS)
                onCancel()
            }
            OutcomeScreen(
                title = stringResource(R.string.pairing_collisionTitle),
                message = stringResource(R.string.pairing_collisionMessage),
                actionLabel = stringResource(R.string.pairing_tryAgainButton),
                onAction = onRetry,
                onCancel = onCancel,
                textLines = PAIRING_TEXT_LINES,
            )
        }
        attempt?.timedOut == true -> {
            LaunchedEffect(Unit) {
                delay(AUTO_DISMISS_DELAY_MS)
                onCancel()
            }
            OutcomeScreen(
                title = stringResource(R.string.pairing_timeoutTitle),
                message = stringResource(R.string.pairing_timeoutMessage),
                actionLabel = stringResource(R.string.pairing_tryAgainButton),
                onAction = onRetry,
                onCancel = onCancel,
                textLines = PAIRING_TEXT_LINES,
            )
        }
        else -> WaitingForDeviceScreen(attempt?.id, service, onCancel, waitingForName = null)
    }
}

/**
 * The step the attempt is in (preparing, connecting to relays, waiting, found a device, or — [waitingForName] set —
 * waiting for that person to confirm too; `call-core` decides which) plus how many relays are connected and how
 * long is left, refreshed every second.
 */
@Composable
private fun WaitingForDeviceScreen(pairingId: String?, service: CameraAgentService?, onCancel: () -> Unit, waitingForName: String?) {
    // Once this side has confirmed (waitingForName != null) the countdown is the wait for the other person, from now.
    val startedAt = remember(waitingForName != null) { System.currentTimeMillis() }
    var now by remember { mutableLongStateOf(System.currentTimeMillis()) }
    LaunchedEffect(Unit) {
        while (true) {
            delay(1_000)
            now = System.currentTimeMillis()
        }
    }
    val relays = remember(now) { service?.signalingSnapshot()?.relays.orEmpty() }
    val connected = relays.count { it.state != CallCoreBridge.RelayState.DOWN }
    val pending = remember(now) { pairingId?.let(CallCoreBridge::pendingSnapshot) }
    val phase = CallCoreBridge.pairingPhase(
        hasAttempt = pending != null,
        relaysConnected = connected,
        candidateFound = pending?.candidatePubkey != null,
        accepted = waitingForName != null,
    )
    val title = when (phase) {
        CallCoreBridge.PairingPhase.PREPARING -> stringResource(R.string.pairing_phasePreparing)
        CallCoreBridge.PairingPhase.CONNECTING -> stringResource(R.string.pairing_phaseConnecting)
        CallCoreBridge.PairingPhase.WAITING -> stringResource(R.string.pairing_waitingForDevice)
        CallCoreBridge.PairingPhase.FOUND -> stringResource(R.string.pairing_phaseFound)
        CallCoreBridge.PairingPhase.WAITING_FOR_CONFIRM ->
            stringResource(R.string.pairing_phaseWaitingForConfirm, waitingForName?.ifBlank { stringResource(R.string.common_thisDevice) }.orEmpty())
    }
    val leftSeconds = ((CallCoreBridge.protocolConstants.pakeLiveWindowMs - (now - startedAt)) / 1000).coerceAtLeast(0)
    // Two short lines once there is something to report, blank while preparing.
    val status = if (phase == CallCoreBridge.PairingPhase.PREPARING) "" else
        stringResource(R.string.pairing_progressRelays, connected, relays.size) + "\n" +
            stringResource(R.string.pairing_progressCancelsIn, "%d:%02d".format(leftSeconds / 60, leftSeconds % 60))
    PairingDialogScreen(title = title, text = status, onBack = onCancel) {
        // Same reasoning as CallingScreen's own spinner — distinguishes "still working" from "stuck."
        CircularProgressIndicator(color = GeneratedColor.colorActionPrimaryBackground)
    }
}

/**
 * SPAKE2 already cryptographically proved both sides typed the same
 * phrase by the time this shows — this tap is a cheap final sanity check
 * (catches the narrow case of one side pairing with the wrong contact by
 * mistake), not the old heavy fingerprint-comparison ceremony. There's
 * nothing left to compare: no code, no fingerprint, just the peer's own
 * self-reported name.
 */
@Composable
internal fun NameConfirmScreen(candidate: CandidatePeer, onConfirm: () -> Unit, onCancel: () -> Unit) {
    val focusRequester = remember { FocusRequester() }
    RequestFocusOnMount(focusRequester)
    PairingDialogScreen(
        title = stringResource(R.string.pairing_confirmTitle, candidate.name.ifBlank { stringResource(R.string.common_thisDevice) }),
        text = stringResource(R.string.pairing_confirmSubtitle),
        onBack = onCancel,
    ) {
        TvButton(onClick = onConfirm, modifier = Modifier.focusRequester(focusRequester)) { Text(stringResource(R.string.pairing_confirmButton)) }
    }
}

/**
 * [CenteredDialogScreen] plus "title + message + one action button, plus a
 * Cancel button below it" — the shape behind every non-success outcome in
 * the app with a real decision to make: pairing collision/timeout (started
 * here) and WaitingScreen's own delete confirmation (HomeScreens.kt).
 * [CallOutcomeScreen] (HomeScreens.kt) is the passive sibling — same
 * [CenteredDialogScreen] shell, no action button, an OK button instead of
 * Cancel. [onCancel] (here [PairingDialogScreen]'s `onBack`) is also
 * reachable via the physical Back key, same as every other screen in this
 * app — but the on-screen button is what a touchscreen Portal, with
 * neither a D-pad nor a hardware Back key, actually has to use.
 */
@Composable
internal fun OutcomeScreen(
    title: String,
    message: String,
    actionLabel: String,
    onAction: () -> Unit,
    onCancel: () -> Unit,
    tint: TvButtonTint = TvButtonTint.Neutral,
    textLines: Int = 1,
) {
    val focusRequester = remember { FocusRequester() }
    RequestFocusOnMount(focusRequester)
    PairingDialogScreen(title = title, text = message, onBack = onCancel, textLines = textLines) {
        TvButton(onClick = onAction, tint = tint, modifier = Modifier.focusRequester(focusRequester)) { Text(actionLabel) }
    }
}

/** Lines of text every pairing dialog reserves, so the action below it sits at the same height on all of them. */
private const val PAIRING_TEXT_LINES = 2

/** Tall enough for the spinner (48 dp by default) and for a [TvButton]. */
private val PairingActionSlotHeight = 48.dp

/**
 * The one shape behind every pairing dialog (progress, collision, timeout,
 * confirm) and, through [OutcomeScreen], the delete confirmation: bright
 * title, dim text, then one action slot holding the spinner or the action
 * button, then a Cancel button below that calls [onBack] — the physical
 * Back key does the same thing, but a touchscreen Portal has neither a
 * D-pad nor a hardware Back key, so the on-screen button is this family's
 * only way out there. The text reserves [textLines] lines and the action
 * slot has a fixed height, so the dialogs are all the same size — and
 * since they are centered, the title and the action sit in the same place
 * on each (web's `.pair-text` / `.pair-action`, which has carried its own
 * on-screen Cancel button all along — see `index.html`'s `progressCancel`/
 * `pairCancel`/`confirmDeleteCancel`).
 */
@Composable
private fun PairingDialogScreen(
    title: String,
    text: String,
    onBack: () -> Unit,
    textLines: Int = PAIRING_TEXT_LINES,
    action: @Composable () -> Unit,
) {
    CenteredDialogScreen(onBack = onBack, backLabel = stringResource(R.string.common_cancel)) {
        // Bright, not dimmed — the one thing the screen exists to say, the same
        // role CallingScreen's contact name plays; matches web's `.panel h1`.
        Text(title, color = GeneratedColor.colorTextPrimary, style = MaterialTheme.typography.headlineSmall, textAlign = TextAlign.Center)
        Text(text, color = GeneratedColor.colorTextDim, style = MaterialTheme.typography.bodyMedium, textAlign = TextAlign.Center, minLines = textLines)
        Box(modifier = Modifier.heightIn(min = PairingActionSlotHeight), contentAlignment = Alignment.Center) { action() }
    }
}
