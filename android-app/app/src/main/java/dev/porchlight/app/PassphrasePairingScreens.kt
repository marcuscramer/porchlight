package dev.porchlight.app

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.style.TextAlign
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
 * The one and only pairing entry screen, for a brand-new contact. Nothing to
 * generate or show/read here, so nothing for either side to be "first" at:
 * both people agree on a phrase together (verbally, during a call) and each
 * just types it into their own device.
 */
@Composable
fun EnterPhraseScreen(onSubmit: (String) -> Unit, onCancel: () -> Unit) {
    BackHandler(onBack = onCancel)
    var phrase by remember { mutableStateOf("") }
    val focusRequester = remember { FocusRequester() }
    RequestFocusOnMount(focusRequester)
    // Shared by the button's onClick and the keyboard's Done action below.
    val submit = { if (phrase.isNotBlank()) onSubmit(phrase) }
    // Same dim-title/centered-content shape every other settings-gear-
    // reachable page screen uses (PageScreen's own doc, MainActivity.kt).
    PageScreen(title = stringResource(R.string.pairing_addContactTitle)) {
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
 * the live window times out with nobody found. Reads [ContactState] live
 * (via [contacts]) rather than owning any state of its own — the same
 * "observe CameraAgentService.state, don't duplicate it" pattern every
 * other screen in this app already uses.
 */
@Composable
fun PairingProgressScreen(
    pairingId: String,
    service: CameraAgentService?,
    contacts: List<CameraAgentService.ContactState>,
    onConfirm: (pairingId: String, publicKeyHex: String) -> Unit,
    onRetry: () -> Unit,
    onCancel: () -> Unit,
) {
    val contact = contacts.find { it.id == pairingId }
    val candidate = contact?.pairingCandidates?.firstOrNull()
    when {
        candidate != null -> NameConfirmScreen(
            candidate = candidate,
            onConfirm = { onConfirm(pairingId, candidate.publicKey) },
            onCancel = onCancel,
        )
        contact?.pairingCollision == true -> {
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
            )
        }
        contact?.pairingTimedOut == true -> {
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
            )
        }
        else -> WaitingForDeviceScreen(pairingId, service, onCancel)
    }
}

/**
 * The step the attempt is in (preparing, connecting to relays, waiting, found a device — `call-core` decides which)
 * plus how many relays are connected and how long is left, refreshed every second.
 */
@Composable
private fun WaitingForDeviceScreen(pairingId: String, service: CameraAgentService?, onCancel: () -> Unit) {
    val startedAt = remember { System.currentTimeMillis() }
    var now by remember { mutableLongStateOf(System.currentTimeMillis()) }
    LaunchedEffect(Unit) {
        while (true) {
            delay(1_000)
            now = System.currentTimeMillis()
        }
    }
    val relays = remember(now) { service?.signalingSnapshot()?.relays.orEmpty() }
    val connected = relays.count { it.state != CallCoreBridge.RelayState.DOWN }
    val pending = remember(now) { CallCoreBridge.pendingSnapshot(pairingId) }
    val phase = CallCoreBridge.pairingPhase(hasAttempt = pending != null, relaysConnected = connected, candidateFound = pending?.candidatePubkey != null)
    val title = when (phase) {
        CallCoreBridge.PairingPhase.PREPARING -> R.string.pairing_phasePreparing
        CallCoreBridge.PairingPhase.CONNECTING -> R.string.pairing_phaseConnecting
        CallCoreBridge.PairingPhase.WAITING -> R.string.pairing_waitingForDevice
        CallCoreBridge.PairingPhase.FOUND -> R.string.pairing_phaseFound
    }
    CenteredDialogScreen(onBack = onCancel) {
        // Same reasoning as CallingScreen's own spinner — distinguishes "still working" from "stuck."
        CircularProgressIndicator(color = GeneratedColor.colorActionPrimaryBackground)
        // Bright, not dimmed — matches web's #progressWaiting h1 (plain .panel h1, --color-text-primary)
        // and this same file's other centered-dialog titles.
        Text(stringResource(title), color = GeneratedColor.colorTextPrimary, style = MaterialTheme.typography.headlineSmall)
        if (phase != CallCoreBridge.PairingPhase.PREPARING) {
            val elapsed = ((now - startedAt) / 1000).coerceAtLeast(0)
            val leftSeconds = ((CallCoreBridge.protocolConstants.pakeLiveWindowMs - (now - startedAt)) / 1000).coerceAtLeast(0)
            Text(
                stringResource(R.string.pairing_progressDetail, connected, relays.size, elapsed, "%d:%02d".format(leftSeconds / 60, leftSeconds % 60)),
                color = GeneratedColor.colorTextDim,
                style = MaterialTheme.typography.bodyMedium,
                textAlign = TextAlign.Center,
            )
        }
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
    CenteredDialogScreen(onBack = onCancel) {
        // Bright, not dimmed — matches web's #pairTitle (.pair-title,
        // --color-text-primary) and this file's other centered-dialog
        // titles.
        Text(
            stringResource(R.string.pairing_confirmTitle, candidate.name.ifBlank { stringResource(R.string.common_thisDevice) }),
            color = GeneratedColor.colorTextPrimary,
            style = MaterialTheme.typography.headlineSmall,
        )
        Text(
            stringResource(R.string.pairing_confirmSubtitle),
            color = GeneratedColor.colorTextDim,
            style = MaterialTheme.typography.bodyMedium,
            textAlign = TextAlign.Center,
        )
        TvButton(onClick = onConfirm, modifier = Modifier.focusRequester(focusRequester)) { Text(stringResource(R.string.pairing_confirmButton)) }
    }
}

/**
 * [CenteredDialogScreen] plus "title + message + one action button" — the
 * shape behind every non-success outcome in the app with a real decision to
 * make: pairing collision/timeout (started here) and WaitingScreen's own
 * delete confirmation (HomeScreens.kt). [CallOutcomeScreen] (HomeScreens.kt)
 * is the passive sibling — same [CenteredDialogScreen] shell, a dismiss
 * hint instead of an action button. [onCancel] is reached only via the
 * physical Back key, same as every other screen in this app — no
 * on-screen Cancel button, deliberately, even for the delete-confirmation
 * case.
 */
@Composable
internal fun OutcomeScreen(
    title: String,
    message: String,
    actionLabel: String,
    onAction: () -> Unit,
    onCancel: () -> Unit,
    tint: TvButtonTint = TvButtonTint.Neutral,
) {
    val focusRequester = remember { FocusRequester() }
    RequestFocusOnMount(focusRequester)
    CenteredDialogScreen(onBack = onCancel) {
        // Bright, not dimmed — this is the one thing the screen exists
        // to say, the same role CallingScreen's contactName/
        // IncomingCallScreen's caller-name Text play (both
        // colorTextPrimary), not the muted "which screen am I on"
        // corner labels (Settings' own title, etc.). Matches web's
        // .panel h1, which has always used --color-text-primary. No
        // alert-red variant; every OutcomeScreen title uses this same
        // color now, regardless of which action it leads to.
        Text(title, color = GeneratedColor.colorTextPrimary, style = MaterialTheme.typography.headlineSmall)
        Text(message, color = GeneratedColor.colorTextDim, style = MaterialTheme.typography.bodyMedium, textAlign = TextAlign.Center)
        TvButton(onClick = onAction, tint = tint, modifier = Modifier.focusRequester(focusRequester)) { Text(actionLabel) }
    }
}
