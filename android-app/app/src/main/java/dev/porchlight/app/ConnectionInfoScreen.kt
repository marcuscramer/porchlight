package dev.porchlight.app

import android.content.Context
import android.net.ConnectivityManager
import android.net.NetworkCapabilities
import android.provider.Settings
import androidx.activity.compose.BackHandler
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontStyle
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.text.withStyle
import androidx.tv.material3.MaterialTheme
import androidx.tv.material3.Text
import dev.porchlight.app.ui.theme.Dimens
import dev.porchlight.app.ui.theme.GeneratedColor
import kotlinx.coroutines.delay

/**
 * A read-only debug page reached from Settings: is the device online, which
 * relays are up, how fresh the traffic is, who's online, and the state of the
 * one-time setup steps. Refreshes every second while open. Back returns to
 * Settings.
 */
@Composable
internal fun ConnectionInfoScreen(
    service: CameraAgentService?,
    state: CameraAgentService.AgentState,
    onBack: () -> Unit,
) {
    BackHandler(onBack = onBack)
    val context = LocalContext.current
    var now by remember { mutableLongStateOf(System.currentTimeMillis()) }
    LaunchedEffect(Unit) {
        while (true) {
            delay(1_000)
            now = System.currentTimeMillis()
        }
    }
    // Re-read every tick (keyed on [now]), not just once on open.
    val snapshot = remember(now) { service?.signalingSnapshot() }
    val wakeUpServiceOn by CallWakeUpAccessibilityService.enabled.collectAsState()
    val overlayAllowed = remember(now) { Settings.canDrawOverlays(context) }
    val selfInstallPossible = remember(now) { UpdateChecker.canSelfInstall(context) }
    val network = remember(now) { networkStatus(context) }

    PageScreen(title = stringResource(R.string.connectionInfo_title), onBack = onBack) {
        Column(verticalArrangement = Arrangement.spacedBy(Dimens.dimension8)) {
            InfoRow(stringResource(R.string.connectionInfo_network), stringResource(network.textRes), if (network.good) OK else BAD)
            InfoRow(stringResource(R.string.connectionInfo_lastHeartbeat), agoText(CallCoreBridge.ago(snapshot?.lastHeartbeatSentAtMs, now)))
            val peers = state.contacts
            InfoRow(
                stringResource(R.string.connectionInfo_contactsOnline),
                stringResource(R.string.connectionInfo_contactsOnlineValue, peers.count { it.status != CallCoreBridge.PresenceStatus.OFFLINE }, peers.size),
            )
            InfoRow(stringResource(R.string.connectionInfo_wakeUpService), stringResource(if (wakeUpServiceOn) R.string.connectionInfo_on else R.string.connectionInfo_off))
            InfoRow(stringResource(R.string.connectionInfo_overlay), stringResource(if (overlayAllowed) R.string.connectionInfo_allowed else R.string.connectionInfo_notAllowed))
            InfoRow(
                stringResource(R.string.connectionInfo_installVerifier),
                stringResource(if (selfInstallPossible) R.string.connectionInfo_off else R.string.connectionInfo_verifierBlocking),
                if (selfInstallPossible) OK else WARN,
            )

            // The relays come last, set apart from the rows above.
            Spacer(Modifier.height(Dimens.dimension16))
            val relays = snapshot?.relays.orEmpty()
            val up = relays.count { it.state != CallCoreBridge.RelayState.DOWN }
            InfoRow(
                stringResource(R.string.connectionInfo_relays),
                stringResource(R.string.connectionInfo_relaysSummary, up, relays.size),
                if (up > 0) OK else BAD,
            )
            for (relay in relays) {
                val dot = when (relay.state) {
                    CallCoreBridge.RelayState.DOWN -> BAD
                    CallCoreBridge.RelayState.PAUSED -> WARN
                    CallCoreBridge.RelayState.CONNECTED -> OK
                }
                val dotDescription = stringResource(if (relay.state == CallCoreBridge.RelayState.DOWN) R.string.connectionInfo_relayNotConnected else R.string.connectionInfo_relayConnected)
                val counts = stringResource(R.string.connectionInfo_relayCounts, relay.accepted, relay.rejected)
                val detail = buildAnnotatedString {
                    append("$counts · ${agoText(relay.ago)}")
                    if (relay.error != null) {
                        append(": ")
                        withStyle(SpanStyle(fontStyle = FontStyle.Italic)) { append(relay.error) }
                    }
                }
                RelayRow(relay.host, detail, dot, dotDescription)
            }

            // The version in use; "checked" only once a check has succeeded.
            val listVersion = remember(now) { CallCoreBridge.relayListCurrent(System.currentTimeMillis()).version }
            val listCheckedAt = service?.relayListStatus()?.checkedAtMs
            InfoRow(
                stringResource(R.string.connectionInfo_relayList),
                if (listCheckedAt == null) "v$listVersion"
                else stringResource(R.string.connectionInfo_relayListValue, listVersion, agoText(CallCoreBridge.ago(listCheckedAt, now))),
            )
            RelayListCheck(service)
        }
    }
}

/** "Check for relay updates" and what came of it. Stays quiet until pressed. */
@Composable
private fun RelayListCheck(service: CameraAgentService?) {
    var checking by remember { mutableStateOf(false) }
    var result by remember { mutableStateOf<RelayListUpdater.Result?>(null) }
    val version = CallCoreBridge.relayListCurrent(System.currentTimeMillis()).version
    // The button and its result on one line, centered as a whole.
    Row(
        modifier = Modifier.fillMaxWidth(),
        horizontalArrangement = Arrangement.spacedBy(Dimens.dimension16, Alignment.CenterHorizontally),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        TvButton(
            onClick = {
                checking = true
                service?.checkRelayListNow { r -> result = r; checking = false }
            },
            enabled = !checking && service != null,
            compact = true,
        ) { Text(stringResource(R.string.connectionInfo_relayListCheck)) }
        val shown = result
        val message = if (checking) stringResource(R.string.connectionInfo_relayListChecking) else when (shown) {
            null -> null
            RelayListUpdater.Result.UpToDate -> stringResource(R.string.connectionInfo_relayListUpToDate, version)
            is RelayListUpdater.Result.Applied -> stringResource(R.string.connectionInfo_relayListApplied, shown.version)
            is RelayListUpdater.Result.Failed -> stringResource(R.string.connectionInfo_relayListFailed, shown.reason)
            RelayListUpdater.Result.Disabled -> stringResource(R.string.connectionInfo_relayListDisabled)
        }
        if (message != null) Text(message, color = GeneratedColor.colorTextPrimary, style = MaterialTheme.typography.bodySmall)
    }
}

private val OK = GeneratedColor.colorStatusOk
private val WARN = GeneratedColor.colorStatusBusy
private val BAD = GeneratedColor.colorStatusDanger

@Composable
private fun InfoRow(label: String, value: String, valueColor: Color = GeneratedColor.colorTextPrimary) {
    Row(
        modifier = Modifier.fillMaxWidth(),
        horizontalArrangement = Arrangement.spacedBy(Dimens.spacingContactRowGap),
    ) {
        Text(label, color = GeneratedColor.colorTextPrimary, style = MaterialTheme.typography.bodySmall, modifier = Modifier.weight(1f))
        Text(value, color = valueColor, style = MaterialTheme.typography.bodySmall, textAlign = TextAlign.End)
    }
}

private data class NetworkStatus(val textRes: Int, val good: Boolean)

private fun networkStatus(context: Context): NetworkStatus {
    val cm = context.getSystemService(Context.CONNECTIVITY_SERVICE) as ConnectivityManager
    val caps = cm.activeNetwork?.let { cm.getNetworkCapabilities(it) }
        ?: return NetworkStatus(R.string.connectionInfo_networkOffline, false)
    if (!caps.hasCapability(NetworkCapabilities.NET_CAPABILITY_INTERNET)) return NetworkStatus(R.string.connectionInfo_networkOffline, false)
    if (!caps.hasCapability(NetworkCapabilities.NET_CAPABILITY_VALIDATED)) return NetworkStatus(R.string.connectionInfo_networkLimited, false)
    val res = when {
        caps.hasTransport(NetworkCapabilities.TRANSPORT_WIFI) -> R.string.connectionInfo_networkWifi
        caps.hasTransport(NetworkCapabilities.TRANSPORT_ETHERNET) -> R.string.connectionInfo_networkEthernet
        caps.hasTransport(NetworkCapabilities.TRANSPORT_CELLULAR) -> R.string.connectionInfo_networkCellular
        else -> R.string.connectionInfo_networkOther
    }
    return NetworkStatus(res, true)
}

/** "3 s ago" / "2 min ago" / "1 h ago", or "never" — the buckets are `call-core`'s. */
@Composable
private fun agoText(ago: CallCoreBridge.Ago): String = when (ago) {
    CallCoreBridge.Ago.Never -> stringResource(R.string.connectionInfo_never)
    is CallCoreBridge.Ago.Seconds -> stringResource(R.string.connectionInfo_agoSeconds, ago.n)
    is CallCoreBridge.Ago.Minutes -> stringResource(R.string.connectionInfo_agoMinutes, ago.n)
    is CallCoreBridge.Ago.Hours -> stringResource(R.string.connectionInfo_agoHours, ago.n)
}

/** A relay line: a small status dot hugging the top-left of the name (as in the contact list), then the one-line detail, ellipsized if long. */
@Composable
private fun RelayRow(host: String, detail: androidx.compose.ui.text.AnnotatedString, dot: Color, dotDescription: String) {
    Row(modifier = Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(Dimens.spacingContactRowGap)) {
        Row(verticalAlignment = Alignment.Top) {
            StatusDot(hue = dot, contentDescription = dotDescription, size = Dimens.dimension6)
            Text(host, color = GeneratedColor.colorTextPrimary, style = MaterialTheme.typography.bodySmall, maxLines = 1)
        }
        Text(
            detail,
            color = GeneratedColor.colorTextPrimary,
            style = MaterialTheme.typography.bodySmall,
            textAlign = TextAlign.End,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
            modifier = Modifier.weight(1f),
        )
    }
}
