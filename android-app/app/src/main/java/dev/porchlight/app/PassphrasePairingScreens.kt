package dev.porchlight.app

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text as M3Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.input.key.Key
import androidx.compose.ui.input.key.KeyEventType
import androidx.compose.ui.input.key.key
import androidx.compose.ui.input.key.onPreviewKeyEvent
import androidx.compose.ui.input.key.type
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.style.TextAlign
import androidx.tv.material3.MaterialTheme
import androidx.tv.material3.Text
import dev.porchlight.app.ui.theme.Dimens
import dev.porchlight.app.ui.theme.GeneratedColor
import dev.porchlight.app.ui.theme.porchlightScreenBackground

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
    LaunchedEffect(Unit) { focusRequester.requestFocus() }
    // Shared by the button's onClick and the keyboard's Done action below.
    val submit = { if (phrase.isNotBlank()) onSubmit(phrase) }
    Column(
        modifier = Modifier
            .fillMaxSize()
            .porchlightScreenBackground()
            .padding(Dimens.spacingScreenPadding)
            .verticalScroll(rememberScrollState()),
        verticalArrangement = Arrangement.spacedBy(Dimens.spacingPanelContentGap),
    ) {
        // Dimmed like every other screen's own title (matches Settings').
        Text(
            stringResource(R.string.pairing_addContactTitle),
            color = GeneratedColor.colorTextDim,
            style = MaterialTheme.typography.headlineSmall,
        )
        Text(
            stringResource(R.string.pairing_instructions1),
            color = GeneratedColor.colorTextDim,
            style = MaterialTheme.typography.bodyMedium,
        )
        Text(
            stringResource(R.string.pairing_instructions2),
            color = GeneratedColor.colorTextDim,
            style = MaterialTheme.typography.bodyMedium,
        )
        Text(
            stringResource(R.string.pairing_instructions3),
            color = GeneratedColor.colorTextDim,
            style = MaterialTheme.typography.bodyMedium,
        )
        OutlinedTextField(
            value = phrase,
            // take(200): generous for "a short phrase," but a real bound —
            // this becomes a passphrase string fed to CallCoreBridge/JNI,
            // and while that path is panic-safe regardless, there's no
            // reason to let an accidental massive paste through.
            onValueChange = { phrase = it.take(200) },
            label = { M3Text(stringResource(R.string.pairing_phraseFieldLabel)) },
            modifier = Modifier.fillMaxWidth().focusRequester(focusRequester).onPreviewKeyEvent { event ->
                // See HardwareEnterKeyUpGuard's own doc (MainActivity.kt):
                // submit()-ing here handles the KeyDown, but the real fix
                // for the accidental-click issue is arm() — Android
                // dispatches this same physical press's KeyUp separately,
                // straight to whatever the *next* screen's first focusable
                // turns out to be.
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
            // "\n" into the phrase, with no way to submit from the keyboard.
            singleLine = true,
            keyboardOptions = KeyboardOptions(imeAction = ImeAction.Done),
            keyboardActions = KeyboardActions(onDone = { submit() }),
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
        contact?.pairingCollision == true -> OutcomeScreen(
            title = stringResource(R.string.pairing_collisionTitle),
            message = stringResource(R.string.pairing_collisionMessage),
            actionLabel = stringResource(R.string.pairing_tryAgainButton),
            titleIsAlert = true,
            onAction = onRetry,
            onCancel = onCancel,
        )
        contact?.pairingTimedOut == true -> OutcomeScreen(
            title = stringResource(R.string.pairing_timeoutTitle),
            message = stringResource(R.string.pairing_timeoutMessage),
            actionLabel = stringResource(R.string.pairing_tryAgainButton),
            onAction = onRetry,
            onCancel = onCancel,
        )
        else -> {
            BackHandler(onBack = onCancel)
            Box(modifier = Modifier.fillMaxSize().porchlightScreenBackground(), contentAlignment = Alignment.Center) {
                Column(
                    horizontalAlignment = Alignment.CenterHorizontally,
                    verticalArrangement = Arrangement.spacedBy(Dimens.spacingPanelContentGap),
                    modifier = Modifier.padding(horizontal = Dimens.spacingScreenPadding),
                ) {
                    // Same reasoning as CallingScreen's own spinner —
                    // distinguishes "still working" from "stuck."
                    CircularProgressIndicator(color = GeneratedColor.colorActionPrimaryBackground)
                    Text(stringResource(R.string.pairing_waitingForDevice), color = GeneratedColor.colorTextDim, style = MaterialTheme.typography.headlineSmall)
                }
            }
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
    BackHandler(onBack = onCancel)
    val focusRequester = remember { FocusRequester() }
    LaunchedEffect(Unit) { focusRequester.requestFocus() }
    Box(modifier = Modifier.fillMaxSize().porchlightScreenBackground(), contentAlignment = Alignment.Center) {
        Column(
            horizontalAlignment = Alignment.CenterHorizontally,
            verticalArrangement = Arrangement.spacedBy(Dimens.spacingPanelContentGap),
            modifier = Modifier.padding(horizontal = Dimens.spacingScreenPadding),
        ) {
            Text(
                stringResource(R.string.pairing_confirmTitle, candidate.name.ifBlank { stringResource(R.string.common_thisDevice) }),
                color = GeneratedColor.colorTextDim,
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
}

/**
 * Shared full-screen "title + message + one action button, Back to
 * dismiss" prompt — the one shape behind every non-success outcome in the
 * app: pairing collision/timeout (started here), call-outcome screens
 * ([CallOutcomeScreen] in HomeScreens.kt), and WaitingScreen's own delete
 * confirmation (HomeScreens.kt). [onCancel] is reached only via the
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
    titleIsAlert: Boolean = false,
    tint: TvButtonTint = TvButtonTint.Neutral,
) {
    BackHandler(onBack = onCancel)
    val focusRequester = remember { FocusRequester() }
    LaunchedEffect(Unit) { focusRequester.requestFocus() }
    Box(modifier = Modifier.fillMaxSize().porchlightScreenBackground(), contentAlignment = Alignment.Center) {
        Column(
            horizontalAlignment = Alignment.CenterHorizontally,
            verticalArrangement = Arrangement.spacedBy(Dimens.spacingPanelContentGap),
            modifier = Modifier.padding(horizontal = Dimens.spacingScreenPadding),
        ) {
            // Dimmed like every other screen's own title (matches
            // Settings') — except a real alert, which keeps its
            // intentional danger-red instead.
            Text(title, color = if (titleIsAlert) GeneratedColor.colorActionDangerBackground else GeneratedColor.colorTextDim, style = MaterialTheme.typography.headlineSmall)
            Text(message, color = GeneratedColor.colorTextDim, style = MaterialTheme.typography.bodyMedium, textAlign = TextAlign.Center)
            TvButton(onClick = onAction, tint = tint, modifier = Modifier.focusRequester(focusRequester)) { Text(actionLabel) }
        }
    }
}
