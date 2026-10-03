package dev.porchlight.app

import android.content.Context
import android.net.ConnectivityManager
import android.net.NetworkCapabilities
import android.provider.Settings
import androidx.activity.compose.BackHandler
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableLongStateOf
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

    PageScreen(title = stringResource(R.string.connectionInfo_title)) {
        Column(verticalArrangement = Arrangement.spacedBy(Dimens.dimension8)) {
            InfoRow(stringResource(R.string.connectionInfo_network), stringResource(network.textRes), if (network.good) OK else BAD)

            val relays = snapshot?.relays.orEmpty()
            val up = relays.count { it.connected }
            InfoRow(
                stringResource(R.string.connectionInfo_relays),
                stringResource(R.string.connectionInfo_relaysSummary, up, relays.size),
                if (up > 0) OK else BAD,
            )
            for (relay in relays) {
                // The dot says it: green = connected and in use, yellow = connected
                // but paused after a rejection, red = not connected.
                val dot = when {
                    !relay.connected -> BAD
                    relay.backingOff -> WARN
                    else -> OK
                }
                val dotDescription = stringResource(if (relay.connected) R.string.connectionInfo_relayConnected else R.string.connectionInfo_relayNotConnected)
                val counts = stringResource(R.string.connectionInfo_relayCounts, relay.accepted, relay.rejected)
                val showError = relay.lastError != null && relay.lastErrorAtMs != null && (!relay.connected || relay.backingOff)
                val whenText = ago(if (showError) relay.lastErrorAtMs else relay.lastMessageAtMs, now)
                val detail = buildAnnotatedString {
                    append("$counts · $whenText")
                    if (showError) {
                        append(": ")
                        withStyle(SpanStyle(fontStyle = FontStyle.Italic)) { append(relay.lastError!!.substringBefore(". Exception:").take(200)) }
                    }
                }
                RelayRow(relay.host, detail, dot, dotDescription)
            }

            InfoRow(stringResource(R.string.connectionInfo_lastHeartbeat), ago(snapshot?.lastHeartbeatSentAtMs, now))
            val peers = state.contacts.filter { it.isPaired }
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
        }
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
        Text(label, color = GeneratedColor.colorTextDim, style = MaterialTheme.typography.bodySmall, modifier = Modifier.weight(1f))
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

/** "3 s ago" / "2 min ago" / "1 h ago", or "never". */
@Composable
private fun ago(atMs: Long?, now: Long): String {
    if (atMs == null) return stringResource(R.string.connectionInfo_never)
    val seconds = ((now - atMs) / 1_000).coerceAtLeast(0)
    return when {
        seconds < 60 -> stringResource(R.string.connectionInfo_agoSeconds, seconds)
        seconds < 3_600 -> stringResource(R.string.connectionInfo_agoMinutes, seconds / 60)
        else -> stringResource(R.string.connectionInfo_agoHours, seconds / 3_600)
    }
}

/** A relay line: a small status dot hugging the top-left of the name (as in the contact list), then the one-line detail, ellipsized if long. */
@Composable
private fun RelayRow(host: String, detail: androidx.compose.ui.text.AnnotatedString, dot: Color, dotDescription: String) {
    Row(modifier = Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(Dimens.spacingContactRowGap)) {
        Row(verticalAlignment = Alignment.Top) {
            StatusDot(hue = dot, contentDescription = dotDescription, size = Dimens.dimension6)
            Text(host, color = GeneratedColor.colorTextDim, style = MaterialTheme.typography.bodySmall, maxLines = 1)
        }
        Text(
            detail,
            color = GeneratedColor.colorTextDim,
            style = MaterialTheme.typography.bodySmall,
            textAlign = TextAlign.End,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
            modifier = Modifier.weight(1f),
        )
    }
}
