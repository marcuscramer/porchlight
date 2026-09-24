package dev.porchlight.app

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.interaction.collectIsFocusedAsState
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxScope
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.IntrinsicSize
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Call
import androidx.compose.material.icons.filled.Settings
import androidx.compose.material3.CircularProgressIndicator
import androidx.tv.material3.Icon
import androidx.tv.material3.LocalContentColor
import androidx.tv.material3.MaterialTheme
import androidx.tv.material3.Switch
import androidx.tv.material3.SwitchDefaults
import androidx.tv.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.movableContentWithReceiverOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.graphics.TransformOrigin
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.graphics.vector.PathNode
import androidx.compose.ui.graphics.vector.PathParser
import androidx.compose.ui.graphics.vector.path
import androidx.compose.ui.layout.layout
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.zIndex
import androidx.compose.ui.viewinterop.AndroidView
import kotlin.math.roundToInt
import dev.porchlight.app.ui.theme.Dimens
import dev.porchlight.app.ui.theme.GeneratedColor
import dev.porchlight.app.ui.theme.GeneratedOpacity
import dev.porchlight.app.ui.theme.Type
import dev.porchlight.app.ui.theme.fadingEdges
import dev.porchlight.app.ui.theme.porchlightScreenBackground
import org.webrtc.RendererCommon
import org.webrtc.SurfaceViewRenderer
import org.webrtc.VideoSink

/**
 * Material Design's own "person_add" glyph (Apache-2.0, from Google's
 * open-source material-design-icons set) — not in this project's
 * material-icons-core dependency, and pulling in material-icons-extended
 * just for one glyph isn't worth it, so it's hand-built here from the same
 * path data that library would ship.
 */
private val PersonAddIcon: ImageVector = ImageVector.Builder(
    name = "PersonAdd",
    defaultWidth = 24.dp,
    defaultHeight = 24.dp,
    viewportWidth = 24f,
    viewportHeight = 24f,
).apply {
    path(fill = SolidColor(Color.Black)) {
        // Head.
        moveTo(15f, 12f)
        curveTo(17.21f, 12f, 19f, 10.21f, 19f, 8f)
        curveTo(19f, 5.79f, 17.21f, 4f, 15f, 4f)
        curveTo(12.79f, 4f, 11f, 5.79f, 11f, 8f)
        curveTo(11f, 10.21f, 12.79f, 12f, 15f, 12f)
        close()
        // "+".
        moveTo(6f, 10f)
        lineTo(6f, 7f)
        lineTo(4f, 7f)
        lineTo(4f, 10f)
        lineTo(1f, 10f)
        lineTo(1f, 12f)
        lineTo(4f, 12f)
        lineTo(4f, 15f)
        lineTo(6f, 15f)
        lineTo(6f, 12f)
        lineTo(9f, 12f)
        lineTo(9f, 10f)
        close()
        // Shoulders.
        moveTo(15f, 14f)
        curveTo(12.33f, 14f, 7f, 15.34f, 7f, 18f)
        lineTo(7f, 20f)
        lineTo(23f, 20f)
        lineTo(23f, 18f)
        curveTo(23f, 15.34f, 17.67f, 14f, 15f, 14f)
        close()
    }
}.build()

/**
 * Translates a raw SVG path `d` string into this builder's own `path {}`
 * DSL calls, node by node — lets an icon be defined from the exact same
 * path data web's inline SVGs use (copy-paste, not hand-transcribed),
 * guaranteeing pixel-shape parity between the two. [PathParser] does the
 * string parsing; this is the missing link back to [ImageVector.Builder].
 */
private fun ImageVector.Builder.addSvgPath(svgPathData: String): ImageVector.Builder {
    val nodes = PathParser().parsePathString(svgPathData).toNodes()
    return path(fill = SolidColor(Color.Black)) {
        for (node in nodes) {
            when (node) {
                is PathNode.MoveTo -> moveTo(node.x, node.y)
                is PathNode.RelativeMoveTo -> moveToRelative(node.dx, node.dy)
                is PathNode.LineTo -> lineTo(node.x, node.y)
                is PathNode.RelativeLineTo -> lineToRelative(node.dx, node.dy)
                is PathNode.HorizontalTo -> horizontalLineTo(node.x)
                is PathNode.RelativeHorizontalTo -> horizontalLineToRelative(node.dx)
                is PathNode.VerticalTo -> verticalLineTo(node.y)
                is PathNode.RelativeVerticalTo -> verticalLineToRelative(node.dy)
                is PathNode.CurveTo -> curveTo(node.x1, node.y1, node.x2, node.y2, node.x3, node.y3)
                is PathNode.RelativeCurveTo -> curveToRelative(node.dx1, node.dy1, node.dx2, node.dy2, node.dx3, node.dy3)
                is PathNode.QuadTo -> quadTo(node.x1, node.y1, node.x2, node.y2)
                is PathNode.RelativeQuadTo -> quadToRelative(node.dx1, node.dy1, node.dx2, node.dy2)
                is PathNode.ReflectiveCurveTo -> reflectiveCurveTo(node.x1, node.y1, node.x2, node.y2)
                is PathNode.RelativeReflectiveCurveTo -> reflectiveCurveToRelative(node.dx1, node.dy1, node.dx2, node.dy2)
                is PathNode.ReflectiveQuadTo -> reflectiveQuadTo(node.x, node.y)
                is PathNode.RelativeReflectiveQuadTo -> reflectiveQuadToRelative(node.dx, node.dy)
                is PathNode.ArcTo -> arcTo(node.horizontalEllipseRadius, node.verticalEllipseRadius, node.theta, node.isMoreThanHalf, node.isPositiveArc, node.arcStartX, node.arcStartY)
                is PathNode.RelativeArcTo -> arcToRelative(node.horizontalEllipseRadius, node.verticalEllipseRadius, node.theta, node.isMoreThanHalf, node.isPositiveArc, node.arcStartDx, node.arcStartDy)
                PathNode.Close -> close()
            }
        }
    }
}

/**
 * A real trash-can silhouette (tapered body, lid, handle, three slats),
 * built via [addSvgPath] from web's identical `#contactList` bin icon SVG
 * path data, for guaranteed pixel-shape parity. The three slats render as
 * real cutouts because their path data winds in the opposite direction
 * from the outer body/lid/handle — [ImageVector.Builder]'s `path {}`
 * defaults to [PathFillType.NonZero], which treats oppositely-wound
 * overlapping subpaths as holes.
 */
private val DeleteBinIcon: ImageVector = ImageVector.Builder(
    name = "DeleteBin",
    defaultWidth = 24.dp,
    defaultHeight = 24.dp,
    viewportWidth = 1024f,
    viewportHeight = 1024f,
).apply {
    addSvgPath(
        "M266.2 256l47.2 581.4c0 32.4 26.2 58.6 58.6 58.6h282c32.4 0 58.6-26.2 58.6-58.6L759.2 256H266.2z m123.2 530L376 320h37l13.8 466h-37.4z m140.6 0h-36V320h36v466z m104.6 0h-37.2l13.6-466H648l-13.4 466zM728 184h-72l-52.6-46c-7.4-6.4-16.8-10-26.4-10h-129.6c-9.8 0-19.4 3.6-26.8 10L368 184h-72c-35.2 0-60 16.8-60 52h552c0-35.2-24.8-52-60-52z",
    )
}.build()

/**
 * Material Design's own "call_end" glyph (Apache-2.0) — same reasoning as
 * [PersonAddIcon] above, built via [addSvgPath] from the web client's
 * identical Decline button SVG path data for pixel-shape parity.
 */
private val CallEndIcon: ImageVector = ImageVector.Builder(
    name = "CallEnd",
    defaultWidth = 24.dp,
    defaultHeight = 24.dp,
    viewportWidth = 24f,
    viewportHeight = 24f,
).apply {
    addSvgPath(
        "M12 9c-1.6 0-3.15.25-4.6.72v3.1c0 .39-.23.74-.56.9-.98.49-1.87 1.12-2.66 1.85-.18.18-.43.28-.7.28-.28 0-.53-.11-.71-.29L.29 13.08c-.18-.17-.29-.42-.29-.7 0-.28.11-.53.29-.71C3.34 8.78 7.46 7 12 7s8.66 1.78 11.71 4.67c.18.18.29.43.29.71 0 .28-.11.53-.29.71l-2.48 2.48c-.18.18-.43.29-.71.29-.27 0-.52-.11-.7-.28-.79-.74-1.69-1.36-2.67-1.85-.33-.16-.56-.5-.56-.9v-3.1C15.15 9.25 13.6 9 12 9z",
    )
}.build()

/**
 * Unlike `Modifier.scale()`, which only shrinks what's *painted* and leaves
 * the pre-scale box reserved in the parent's layout, this also reports the
 * scaled-down size upward, so a Column actually tightens around the
 * smaller result instead of centering it inside room reserved for the
 * original size.
 */
private fun Modifier.scaledSize(scale: Float): Modifier = layout { measurable, constraints ->
    val placeable = measurable.measure(constraints)
    val width = (placeable.width * scale).roundToInt()
    val height = (placeable.height * scale).roundToInt()
    layout(width, height) {
        placeable.placeRelativeWithLayer(0, 0) {
            scaleX = scale
            scaleY = scale
            transformOrigin = TransformOrigin(0f, 0f)
        }
    }
}

// ---------------------------------------------------------------------------
// The "app is set up and running" screens — the normal-operation half of
// AppRoot's `when`, as opposed to SetupScreens.kt's onboarding/admin flows
// and MainActivity.kt's own AdminChoiceScreen/NameEntryScreen. Split out
// purely for file size/navigability; no behavior here depends on the split.
// ---------------------------------------------------------------------------

/**
 * Three states, not two, now that calling is always explicit: idle
 * (contact list), calling (self-view only, waiting on a peer who may not
 * even be reachable yet — no timeout, see CallingScreen's doc), and
 * connected (full remote video). Call is always available and just takes
 * however long it takes — no internet/permission/signaling pre-check here:
 * a pre-check would only ever read cached state, not request anything,
 * so blocking on it would leave no way to actually grant a missing
 * permission — the real camera/mic prompt only fires from inside a real
 * call attempt (WebRtcEngine's actual getUserMedia-equivalent), which a
 * pre-check would prevent from ever running.
 */
@Composable
internal fun HomeScreen(
    service: CameraAgentService?,
    state: CameraAgentService.AgentState,
    config: Config,
    onCyclePreviewPosition: () -> Unit,
    onOpenSettings: () -> Unit,
    onAddContact: () -> Unit,
    onDeleteContact: (String) -> Unit,
    onToggleAutoAnswer: (String, Boolean) -> Unit,
) {
    val activeContact = state.contacts.find { it.id == state.activePairingId }
    val peerConnectedOk = activeContact?.connected == true
    val incoming = state.incomingCall

    // Hoisted up here (not `remember`ed inside the connected branch below)
    // so this screen's own BackHandler, registered once at this level, can
    // see it too. Keyed on activePairingId, not a bare Unit, so this starts
    // fresh every new call with no separate reset effect needed and never
    // flashes a stale `true` from a just-ended call.
    var showCallControls by remember(state.activePairingId) { mutableStateOf(false) }

    // Back is however you get off a call now, whether it's a pending
    // outgoing attempt, a not-yet-accepted incoming one (declines it), or
    // already connected (ends it) — see CameraAgentService.hangUp's doc.
    // While the connected call's own control row is up, Back dismisses
    // *that* first instead of hanging up in the same stroke.
    //
    // Always enabled, not just while a call's active: this is a resident
    // device, not an app you navigate away from — with nothing else to
    // intercept it, an idle Back press would otherwise fall through to the
    // system default and exit straight to the launcher. Idle (no
    // activeContact) is a no-op, absorbing the press instead.
    BackHandler(enabled = true) {
        when {
            activeContact == null -> {}
            showCallControls -> showCallControls = false
            else -> service?.hangUp()
        }
    }

    // The self-view, as ONE composable definition for the whole ringing→
    // connected lifecycle of a call — not one embedded in
    // IncomingCallScreen, another in CallingScreen, and a third in the
    // connected branch below, each a separate SurfaceViewRenderer torn
    // down and recreated at every screen transition. Found live on real
    // Portal hardware: that churn raced the old renderer's async EGL
    // teardown against the new one's init on the one shared
    // EglBase.Context, leaving the video surface permanently unattached
    // ("Dropping frame - No surface" for the rest of the call, every
    // time). movableContentOf, not just hoisting the call site: this
    // same underlying node needs to actually sit at a *different
    // position* in the tree depending on the branch below (behind
    // CallingScreen/IncomingCallScreen's own text overlay while ringing,
    // but *above* RemoteVideoView once connected) — found live, again,
    // that a plain Modifier.zIndex() does not control stacking between
    // two setZOrderMediaOverlay(true) hardware-overlay SurfaceViews the
    // way it does for ordinary Compose content; only real declaration
    // order (matching the original, always-worked ordering: self-view
    // added after RemoteVideoView) does. movableContentOf relocates the
    // same renderer to whichever position is actually invoked below,
    // with no dispose/recreate either way.
    val selfView = remember {
        movableContentWithReceiverOf<BoxScope> {
            val previewModifier = if (peerConnectedOk) {
                Modifier
                    .align(config.previewCorner.toAlignment())
                    .padding(Dimens.spacingContactRowGap)
                    .size(Dimens.sizeLocalPreviewWidth, Dimens.sizeLocalPreviewHeight)
            } else {
                Modifier.fillMaxSize()
            }
            LocalPreviewView(service = service, ready = state.capturing, modifier = previewModifier)
        }
    }

    Box(modifier = Modifier.fillMaxSize()) {
        when {
            state.pendingCallOutcome != null -> {
                val outcome = state.pendingCallOutcome
                val name = state.contacts.find { it.id == outcome.pairingId }?.name?.ifBlank { "Unnamed contact" } ?: "Unnamed contact"
                CallOutcomeScreen(
                    contactName = name,
                    reason = outcome.reason,
                    onDismiss = { service?.dismissCallOutcome() },
                )
            }
            activeContact == null -> {
                // No SurfaceView at all while waiting (selfView above
                // isn't invoked anywhere in this branch) — a small corner
                // self-view alongside plain Compose UI (with no
                // full-screen SurfaceView to go with it) blanks that UI on
                // this hardware's compositor; not worth chasing for a
                // nice-to-have.
                WaitingScreen(
                    contacts = state.contacts,
                    pairings = config.pairings,
                    onCall = { pairingId -> service?.requestCall(pairingId) },
                    onOpenSettings = onOpenSettings,
                    onAddContact = onAddContact,
                    onDeleteContact = onDeleteContact,
                    onToggleAutoAnswer = onToggleAutoAnswer,
                )
            }
            incoming != null && !peerConnectedOk -> IncomingCallScreen(
                contactName = activeContact.name,
                info = incoming,
                onAccept = { service?.acceptIncomingCall() },
                service = service,
                selfView = selfView,
            )
            !peerConnectedOk -> CallingScreen(
                contactName = activeContact.name,
                label = if (state.acceptedIncoming) "Connecting" else "Calling",
                selfView = selfView,
            )
            else -> {
                // Full remote video. Rectangular, not rounded:
                // SurfaceViewRenderer's video content is a hardware
                // overlay that ignores Compose's clip() entirely, so a
                // rounded clip only rounds the (invisible) empty corners
                // while the square video underneath still shows through
                // them.
                RemoteVideoView(service = service, ready = state.running)
                // Declared (and thus native-view-added) after
                // RemoteVideoView above — see selfView's own doc for why
                // that ordering, not zIndex, is what actually puts it on
                // top on this hardware. Not invoked at all while cycled
                // to INVISIBLE — a real, user-triggered dispose/recreate
                // point if cycled back on, same as the waiting-screen case
                // above, and unchanged from before this refactor.
                if (config.previewCorner != PreviewCorner.INVISIBLE) selfView()

                // Nothing else is drawn over the video until this is
                // explicitly asked for — select this full-screen surface
                // (D-pad center/remote OK) to reveal Position self-view/
                // Audio/Video/Disconnect below; BackHandler above dismisses
                // them again.
                val focusRequester = remember { FocusRequester() }
                // Fresh every time we return to this branch (a new instance,
                // same as before) — claim focus explicitly rather than
                // relying on whatever Android would otherwise default to.
                LaunchedEffect(Unit) { focusRequester.requestFocus() }
                Box(
                    modifier = Modifier
                        .fillMaxSize()
                        .focusRequester(focusRequester)
                        .clickable(
                            interactionSource = remember { MutableInteractionSource() },
                            indication = null,
                            onClick = { showCallControls = true },
                        ),
                )

                if (showCallControls) {
                    CallControlsOverlay(
                        previewCorner = config.previewCorner,
                        audioEnabled = state.audioEnabled,
                        videoEnabled = state.videoEnabled,
                        onCyclePreviewPosition = onCyclePreviewPosition,
                        onToggleAudio = { service?.setAudioEnabled(!state.audioEnabled) },
                        onToggleVideo = { service?.setVideoEnabled(!state.videoEnabled) },
                        onDisconnect = { service?.hangUp() },
                        // Explicit zIndex so this stays reachable/visible
                        // even when the self-view sits in the same bottom
                        // corner — unlike the RemoteVideoView/self-view
                        // relationship (see selfView's own doc), both this
                        // and the self-view are ordinary Compose content
                        // by this point, so zIndex works normally here.
                        modifier = Modifier.align(Alignment.BottomCenter).zIndex(2f),
                    )
                }
            }
        }
    }
}

/**
 * A translucent black backdrop behind an overlay's own text/buttons — a
 * video feed behind it can be anything, so contrast can't be guaranteed by
 * this screen's own palette. opacity50 over black, not a fully opaque
 * card — still reads as "video underneath," just legible. Mirrors web's
 * identical `.calling-overlay` background (styles.css).
 */
private fun Modifier.callOverlayScrim(): Modifier = this
    .background(Color.Black.copy(alpha = GeneratedOpacity.opacity50), RoundedCornerShape(Dimens.radiusControl))
    .padding(horizontal = Dimens.dimension24, vertical = Dimens.dimension16)

/**
 * The connected call's own on-demand control row — Position self-view/
 * Audio/Video/Disconnect, hidden until HomeScreen's full-screen surface is
 * selected (see its own doc). Mirrors web's identical #callControls
 * (app.js/styles.css) control-for-control.
 */
@Composable
private fun CallControlsOverlay(
    previewCorner: PreviewCorner,
    audioEnabled: Boolean,
    videoEnabled: Boolean,
    onCyclePreviewPosition: () -> Unit,
    onToggleAudio: (Boolean) -> Unit,
    onToggleVideo: (Boolean) -> Unit,
    onDisconnect: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val focusRequester = remember { FocusRequester() }
    LaunchedEffect(Unit) { focusRequester.requestFocus() }
    Row(
        modifier = modifier.padding(bottom = Dimens.spacingContactRowGap).callOverlayScrim(),
        horizontalArrangement = Arrangement.spacedBy(Dimens.dimension16),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        // Icon-only — see PositionSelfViewIcon's own doc for why it's a
        // live drawing rather than a static ImageVector.
        TvButton(
            onClick = onCyclePreviewPosition,
            modifier = Modifier.focusRequester(focusRequester),
        ) {
            PositionSelfViewIcon(corner = previewCorner, modifier = Modifier.size(Dimens.dimension20))
        }
        CallToggle(label = "Audio", checked = audioEnabled, onCheckedChange = onToggleAudio, contentDescription = "Audio")
        CallToggle(label = "Video", checked = videoEnabled, onCheckedChange = onToggleVideo, contentDescription = "Video")
        // Same shape/tint/icon as IncomingCallScreen's own Decline button.
        TvButton(onClick = onDisconnect, tint = TvButtonTint.Danger) {
            Icon(CallEndIcon, contentDescription = "Disconnect", modifier = Modifier.size(Dimens.dimension20))
        }
    }
}

/**
 * Label above a toggle switch — the same visual pattern as ContactRow's
 * own Auto-answer control (see its own doc).
 */
@Composable
private fun CallToggle(label: String, checked: Boolean, onCheckedChange: (Boolean) -> Unit, contentDescription: String) {
    val interactionSource = remember { MutableInteractionSource() }
    val focused by interactionSource.collectIsFocusedAsState()
    Column(horizontalAlignment = Alignment.CenterHorizontally) {
        Text(
            label,
            color = if (focused) Color.White else GeneratedColor.colorTextDim,
            style = Type.statusRow.copy(fontSize = Type.statusRow.fontSize * 0.85f),
            modifier = Modifier.padding(bottom = Dimens.dimension4),
        )
        Switch(
            checked = checked,
            onCheckedChange = onCheckedChange,
            colors = SwitchDefaults.colors(
                checkedThumbColor = GeneratedColor.colorTextPrimary,
                checkedTrackColor = GeneratedColor.colorStatusOk,
                checkedBorderColor = GeneratedColor.colorStatusOk,
                uncheckedThumbColor = GeneratedColor.colorTextDim,
                uncheckedTrackColor = GeneratedColor.colorBackgroundSurfaceAlt,
                uncheckedBorderColor = GeneratedColor.colorBorderDefault,
            ),
            interactionSource = interactionSource,
            modifier = Modifier
                .scaledSize(0.4f)
                .semantics { this.contentDescription = contentDescription },
        )
    }
}

/** Each corner square's own size, in PositionSelfViewIcon's own 24-unit
 * space — sized so its outward corner lands exactly on the outer square's
 * edge while its inward corner stays at 10 or 14. Mirrors web's identical
 * PREVIEW_ICON_SQUARE_SIZE (app.js). */
private const val POSITION_ICON_SQUARE_SIZE = 8f

/** Where each corner's square sits, scaled to the actual Canvas size at
 * draw time — mirrors web's identical PREVIEW_ICON_SQUARES (app.js). Shared
 * by both the filled (active) and outlined (inactive) variant. */
private val POSITION_SQUARES = mapOf(
    PreviewCorner.TOP_START to Offset(2f, 2f),
    PreviewCorner.TOP_END to Offset(14f, 2f),
    PreviewCorner.BOTTOM_START to Offset(2f, 14f),
    PreviewCorner.BOTTOM_END to Offset(14f, 14f),
)

/** Stroke width for both the outer square and each inactive corner square —
 * shared so the inset math below (see PositionSelfViewIcon) can size an
 * outlined square's own rect correctly against it. Mirrors web's identical
 * PREVIEW_ICON_STROKE_WIDTH (app.js). */
private const val POSITION_ICON_STROKE_WIDTH = 1.3f

/**
 * An outer square (sharp corners) with a square at each corner: the one
 * matching [corner] filled solid, the rest outlined, sized so an
 * *outlined* square's stroke lands exactly on the same footprint a
 * *filled* one occupies — Compose centers a stroke on its path by default,
 * so a plain full-size outlined rect at this stroke-width would paint
 * larger than a filled rect of the same nominal size by half the
 * stroke-width on every side; inset by that amount first so the stroke's
 * own outer edge lands on the true bounds. Mirrors web's identical
 * buildPositionIconInner (app.js) control-for-control. A live drawing
 * rather than a static ImageVector since which corner is filled has to
 * track this screen's own live state.
 */
@Composable
private fun PositionSelfViewIcon(corner: PreviewCorner, modifier: Modifier = Modifier) {
    val color = LocalContentColor.current
    Canvas(modifier = modifier) {
        // A single unit for both axes (the Canvas is always square) so
        // POSITION_SQUARES' 24-unit coordinates carry over unscaled, the
        // same space web's own 24x24 SVG viewBox uses.
        val unit = size.width / 24f
        drawRect(
            color = color,
            topLeft = Offset(2f * unit, 2f * unit),
            size = Size(20f * unit, 20f * unit),
            style = Stroke(1.5f * unit),
        )
        for ((c, square) in POSITION_SQUARES) {
            if (c == corner) {
                drawRect(
                    color = color,
                    topLeft = Offset(square.x * unit, square.y * unit),
                    size = Size(POSITION_ICON_SQUARE_SIZE * unit, POSITION_ICON_SQUARE_SIZE * unit),
                )
            } else {
                val inset = POSITION_ICON_STROKE_WIDTH / 2f
                val edge = POSITION_ICON_SQUARE_SIZE - POSITION_ICON_STROKE_WIDTH
                drawRect(
                    color = color,
                    topLeft = Offset((square.x + inset) * unit, (square.y + inset) * unit),
                    size = Size(edge * unit, edge * unit),
                    style = Stroke(POSITION_ICON_STROKE_WIDTH * unit),
                )
            }
        }
    }
}

/**
 * Shown from the instant "Call" is tapped until either the call connects
 * (HomeScreen then swaps to the full remote-video view above) or it's
 * canceled. [selfView] is HomeScreen's own single, hoisted self-view
 * content (see its own doc for why it's passed in rather than created
 * here) — placed first, so it sits behind this screen's own text/spinner
 * overlay, since the peer might not be reachable yet, and
 * CallCoreBridge.requestCall's doc explains why that's allowed to just
 * take as long as it takes rather than timing out on a guessed clock.
 * "Press Back to cancel" as plain text rather than a focused on-screen
 * button — matches every other screen in this app, which all rely on
 * the physical Back key rather than a dedicated Cancel target.
 */
@Composable
private fun CallingScreen(contactName: String, label: String = "Calling", selfView: @Composable BoxScope.() -> Unit) {
    Box(modifier = Modifier.fillMaxSize()) {
        selfView()
        Column(
            modifier = Modifier.align(Alignment.BottomCenter).padding(bottom = Dimens.spacingCallOverlayOffset).callOverlayScrim(),
            horizontalAlignment = Alignment.CenterHorizontally,
            verticalArrangement = Arrangement.spacedBy(Dimens.spacingCallOverlayGap),
        ) {
            // Real feedback that something's still actively happening —
            // this same text stays up through the whole WebRTC negotiation
            // window, so without this a stall here looks identical to a
            // frozen app.
            CircularProgressIndicator(color = GeneratedColor.colorActionPrimaryBackground)
            // [label] distinguishes "we placed this call" from "we just
            // accepted an incoming one" (see AgentState.acceptedIncoming) —
            // otherwise identical negotiating-state screen either way.
            // "Calling" mirrors web's identical .calling-label/.calling-name
            // split (index.html).
            Text(label, color = GeneratedColor.colorTextDim, style = MaterialTheme.typography.bodyMedium, textAlign = TextAlign.Center)
            Text(
                contactName.ifBlank { "Unnamed contact" },
                color = GeneratedColor.colorTextPrimary,
                style = MaterialTheme.typography.headlineSmall,
                textAlign = TextAlign.Center,
            )
            Text("Press Back to cancel", color = GeneratedColor.colorTextDim, style = MaterialTheme.typography.bodyMedium)
        }
    }
}

/**
 * Shown while an incoming call is ringing (manual-accept contact) or
 * counting down (auto-answer contact) — from the instant the offer
 * arrives until it's applied. Self-view behind this is HomeScreen's own
 * hoisted `LocalPreviewView`, same reasoning as CallingScreen's own doc.
 * "Press Back to decline" mirrors CallingScreen's "Press Back to cancel"
 * — Back calls the same `service?.hangUp()` either way (see its own doc
 * for why that's correct for a not-yet-answered incoming call too).
 */
@Composable
private fun IncomingCallScreen(
    contactName: String,
    info: CameraAgentService.IncomingCall,
    onAccept: () -> Unit,
    service: CameraAgentService?,
    selfView: @Composable BoxScope.() -> Unit,
) {
    val name = contactName.ifBlank { "Unnamed contact" }
    Box(modifier = Modifier.fillMaxSize()) {
        selfView()
        Column(
            modifier = Modifier.align(Alignment.BottomCenter).padding(bottom = Dimens.spacingCallOverlayOffset).callOverlayScrim(),
            horizontalAlignment = Alignment.CenterHorizontally,
            verticalArrangement = Arrangement.spacedBy(Dimens.spacingCallOverlayGap),
        ) {
            if (info.autoAnswer) {
                val seconds = info.secondsRemaining
                Text(
                    "$name will automatically connect in $seconds second${if (seconds == 1) "" else "s"}",
                    color = GeneratedColor.colorTextPrimary,
                    style = MaterialTheme.typography.headlineSmall,
                )
            } else {
                Text("Incoming call from", color = GeneratedColor.colorTextDim, style = MaterialTheme.typography.bodyMedium, textAlign = TextAlign.Center)
                Text(name, color = GeneratedColor.colorTextPrimary, style = MaterialTheme.typography.headlineSmall, textAlign = TextAlign.Center)
                val focusRequester = remember { FocusRequester() }
                LaunchedEffect(Unit) { focusRequester.requestFocus() }
                // Accept mirrors the waiting screen's own Call button
                // styling (TvButtonTint.Success + Icons.Filled.Call),
                // Decline is the same shape with Danger + CallEndIcon.
                // "Press Back to decline" below still works the same way —
                // this button is just the discoverable, non-hidden
                // equivalent of it.
                Row(horizontalArrangement = Arrangement.spacedBy(Dimens.spacingCallOverlayGap)) {
                    TvButton(onClick = onAccept, tint = TvButtonTint.Success, modifier = Modifier.focusRequester(focusRequester)) {
                        Icon(Icons.Filled.Call, contentDescription = "Accept", modifier = Modifier.size(Dimens.dimension20))
                    }
                    TvButton(onClick = { service?.hangUp() }, tint = TvButtonTint.Danger) {
                        Icon(CallEndIcon, contentDescription = "Decline", modifier = Modifier.size(Dimens.dimension20))
                    }
                }
            }
            Text("Press Back to decline", color = GeneratedColor.colorTextDim, style = MaterialTheme.typography.bodyMedium)
        }
    }
}

/**
 * Shown whenever a call ends for a reason the person didn't just cause
 * themselves (peer hung up, never connected, or dropped mid-call) — the
 * "why did this call end" decision itself is made once in call-core (see
 * CallOutcomeReason's own doc there). No buttons — Back alone dismisses
 * this, the same "Press Back to ___" pattern CallingScreen/
 * IncomingCallScreen already use.
 */
@Composable
private fun CallOutcomeScreen(
    contactName: String,
    reason: CallCoreBridge.CallOutcomeReason,
    onDismiss: () -> Unit,
) {
    val name = contactName.ifBlank { "Unnamed contact" }
    val (title, message) = when (reason) {
        CallCoreBridge.CallOutcomeReason.PEER_ENDED -> "Call ended" to "$name ended the call."
        CallCoreBridge.CallOutcomeReason.NEVER_CONNECTED -> "Couldn't connect" to
            "The call with $name never connected. Check that both devices have a working internet connection, then try again."
        CallCoreBridge.CallOutcomeReason.DROPPED -> "Call dropped" to
            "The call with $name disconnected unexpectedly — usually just a brief network issue."
    }
    BackHandler(onBack = onDismiss)
    Box(modifier = Modifier.fillMaxSize().porchlightScreenBackground(), contentAlignment = Alignment.Center) {
        Column(
            horizontalAlignment = Alignment.CenterHorizontally,
            verticalArrangement = Arrangement.spacedBy(Dimens.spacingPanelContentGap),
            modifier = Modifier.padding(horizontal = Dimens.spacingScreenPadding),
        ) {
            // Dimmed like every other screen's own title now, not the
            // app's brightest text (matches Settings' own title).
            Text(title, color = GeneratedColor.colorTextDim, style = MaterialTheme.typography.headlineSmall)
            Text(message, color = GeneratedColor.colorTextDim, style = MaterialTheme.typography.bodyMedium, textAlign = TextAlign.Center)
            Text("Press Back to continue", color = GeneratedColor.colorTextDim, style = MaterialTheme.typography.bodyMedium)
        }
    }
}

/**
 * Contacts is the main content of this screen, centered — who you can call,
 * and whether each is currently reachable. Nothing about this device's own
 * internet/permission/relay state is shown here at all.
 *
 * A row shows a badge only for one state that needs explaining — Busy (no
 * button, wait it out). The ordinary case (paired, not currently
 * connected) shows no badge at all: just the name and a Call button,
 * always available regardless of whether the contact is actually online
 * right now — see CallCoreBridge.requestCall's doc: tapping Call no longer
 * requires the peer to already be reachable, it just takes longer if
 * they're not.
 *
 * There's no "needs verification" state anymore — a SPAKE2-confirmed
 * candidate is only ever surfaced live, on [PairingProgressScreen] itself,
 * never as something that lingers here waiting to be picked back up later.
 */
@Composable
private fun WaitingScreen(
    contacts: List<CameraAgentService.ContactState>,
    pairings: List<Pairing>,
    onCall: (String) -> Unit,
    onOpenSettings: () -> Unit,
    onAddContact: () -> Unit,
    onDeleteContact: (String) -> Unit,
    onToggleAutoAnswer: (String, Boolean) -> Unit,
) {
    // Delete is destructive and irreversible (the pairing's own key is
    // gone, not just unlinked) — confirm before actually removing it.
    var pendingDelete by remember { mutableStateOf<CameraAgentService.ContactState?>(null) }
    val toDelete = pendingDelete
    if (toDelete != null) {
        // A real `if`/`else` with the main screen below, not two separately
        // composed siblings: with both composed at once, Compose's D-pad
        // focus-search is purely spatial and has no idea one layer is
        // painted over another — a stray D-pad press with nowhere to go
        // inside this dialog could jump focus onto a completely invisible
        // button from the screen underneath and fire it with no visual
        // indication anything was about to happen.
        OutcomeScreen(
            title = "Delete ${toDelete.name.ifBlank { "this contact" }}?",
            message = "This can't be undone.",
            actionLabel = "Delete",
            tint = TvButtonTint.Danger,
            onAction = { onDeleteContact(toDelete.id); pendingDelete = null },
            onCancel = { pendingDelete = null },
        )
        return
    }
    // Two overlaid corners/center (settings icon / contact list), not a
    // Column sized around each other — the contact list fills the entire
    // screen and the settings icon floats on top of it, same as every
    // other overlaid corner in this app. Mirrors web's identical
    // #screenWaiting restructuring (styles.css).
    BoxWithConstraints(
        modifier = Modifier.fillMaxSize().porchlightScreenBackground(),
    ) {
        // Captured into a plain local (distinct name — a same-named `val
        // maxHeight = maxHeight` would self-reference) — the nested Box
        // below has its own BoxScope receiver, which shadows this one's
        // implicit `maxHeight`.
        val listMaxHeight = maxHeight
        // Mirrors the web client's own #settingsBtn gear. A real
        // ImageVector, not a Unicode glyph — renders consistently on
        // Android's system font, unlike "⚙".
        val settingsFocusRequester = remember { FocusRequester() }
        val settingsInteractionSource = remember { MutableInteractionSource() }
        val settingsFocused by settingsInteractionSource.collectIsFocusedAsState()
        Box(
            modifier = Modifier
                .align(Alignment.TopEnd)
                // This Box is sizeSettingsFab (44dp), bigger than the 24dp
                // icon it centers, so the icon's own ink sits an extra
                // (44-24)/2=10dp deeper than the padding value alone
                // suggests — confirmed live. Subtracting that centering gap
                // is what actually matches the offset used elsewhere in
                // this corner (web's .settings-fab applies the identical
                // correction).
                .padding(Dimens.spacingStatusRegionLeftOffset - (Dimens.sizeSettingsFab - Dimens.dimension24) / 2)
                .size(Dimens.sizeSettingsFab)
                .focusRequester(settingsFocusRequester)
                .clickable(interactionSource = settingsInteractionSource, indication = null, onClick = onOpenSettings),
            contentAlignment = Alignment.Center,
        ) {
            Icon(
                Icons.Filled.Settings,
                contentDescription = "Device settings",
                // Pure white on focus, deliberately a step brighter than
                // colorTextPrimary (which stays as-is for its other uses).
                tint = if (settingsFocused) Color.White else GeneratedColor.colorTextDim,
                modifier = Modifier.size(Dimens.dimension24),
            )
        }
        val listScroll = rememberScrollState()
        // Center + fillMaxSize now — the list fills the entire
        // BoxWithConstraints, so maxHeight below is genuinely the full
        // screen height.
        Box(
            modifier = Modifier.fillMaxSize(),
            contentAlignment = Alignment.Center,
        ) {
            Column(
                verticalArrangement = Arrangement.spacedBy(Dimens.spacingContactListGap),
                modifier = Modifier
                    .padding(horizontal = Dimens.listSideMargin)
                    // Every row below fillMaxWidth()s to this Column's own
                    // width — width(IntrinsicSize.Max) resolves that to the
                    // widest row's natural content width, so Call/
                    // Auto-answer/Delete land at the same x on every row.
                    .width(IntrinsicSize.Max)
                    // heightIn caps this Column so verticalScroll+
                    // fadingEdges take over instead of overflowing past the
                    // screen.
                    .heightIn(max = listMaxHeight)
                    // fadingEdges *before* verticalScroll, not after: a
                    // modifier chain wraps outside-in, so fadingEdges must
                    // wrap verticalScroll from the outside to measure
                    // against the fixed viewport (heightIn's capped
                    // maxHeight) rather than the Column's full unclipped
                    // content height — otherwise its gradient anchors to
                    // the content's own top/bottom, not the viewport's, and
                    // scrolling past the first row or two shows only the
                    // gradient's clamped opaque tail.
                    // bottomHeight is a plain literal, not
                    // Dimens.listFadeHeightBottom (100dp) — that generated
                    // value was sized to clear the status checklist that
                    // used to occupy this corner; nothing does anymore, so
                    // this is just a modest, purely cosmetic edge fade now
                    // (see fadingEdges' own doc for why top/bottom are
                    // shaped differently).
                    .fadingEdges(topHeight = Dimens.listFadeHeightTop, bottomHeight = 48.dp)
                    .verticalScroll(listScroll),
            ) {
                // The first of this Column's two fade spacers — see
                // fadingEdges' own doc (Theme.kt) for why an empty row
                // here, not scroll-position tracking, keeps the permanent
                // top fade from showing when there's nothing above to
                // scroll to.
                Spacer(modifier = Modifier.height(Dimens.listFadeHeightTop))
                // Read once for the whole list, not once per contact — it
                // can't change between one row and the next within the same
                // composition.
                val anyCallActive = CallCoreBridge.isCallActive()
                for (contact in contacts) {
                    ContactRow(
                        name = contact.name,
                        // Call is always offered for a paired,
                        // not-currently-connected contact — see this
                        // screen's doc for why. !anyCallActive is
                        // belt-and-suspenders: tapping Call on a different
                        // contact while already on/dialing a call is a
                        // silent no-op at the call_arbitration layer,
                        // currently unreachable only because this screen
                        // doesn't render while a call is active, not
                        // because anything enforces it.
                        canCall = contact.isPaired && !contact.connected && !anyCallActive,
                        status = contact.status,
                        onCall = { onCall(contact.id) },
                        autoAnswer = pairings.find { it.id == contact.id }?.autoAnswer == true,
                        onToggleAutoAnswer = { enabled -> onToggleAutoAnswer(contact.id, enabled) },
                        onDelete = { pendingDelete = contact },
                    )
                }
                // A blank row, not just a bigger gap — sets "Add contact"
                // apart as its own action rather than one more entry in the
                // contact list, mirroring the web client's identical spacer.
                Spacer(modifier = Modifier.height(Dimens.dimension1))
                // Trailing row, not a separate Settings destination.
                // Centered rather than docked to the list's left edge —
                // this is a standalone action, not one more row of content.
                TvButton(onClick = onAddContact, modifier = Modifier.align(Alignment.CenterHorizontally)) {
                    Icon(PersonAddIcon, contentDescription = "Add contact", modifier = Modifier.size(Dimens.dimension20))
                }
                // Found live: with nothing after it, this button's own
                // border sat flush against verticalScroll's own clip
                // bounds (which clips unconditionally, even when content
                // doesn't need scrolling) — a border centered on its
                // shape's edge overflows by half its own width, so the
                // bottom row of the border was getting cut. A couple dp of
                // trailing slack is cheaper than fighting the clip itself.
                Spacer(modifier = Modifier.height(Dimens.dimension4))
                // The second of this Column's two fade spacers — see the
                // top one's own doc, and fadingEdges' (Theme.kt). Must
                // match the 48.dp passed to fadingEdges' bottomHeight above.
                Spacer(modifier = Modifier.height(48.dp))
            }
        }
    }
}

/**
 * One entry in the contacts list — a badge only for the one exceptional
 * state (Busy); otherwise (paired, not connected) → Call, unconditionally.
 * There's no "needs verification" state to show anymore — a pairing
 * attempt either finishes with the human's "Pair with [name]?" tap right
 * there on [PairingProgressScreen], or it's cancelled and gone. Auto-answer
 * + Delete live directly on this row now, next to the contact each one
 * governs.
 */
@Composable
private fun ContactRow(
    name: String,
    canCall: Boolean,
    status: CallCoreBridge.PresenceStatus,
    onCall: () -> Unit,
    autoAnswer: Boolean,
    onToggleAutoAnswer: (Boolean) -> Unit,
    onDelete: () -> Unit,
) {
    val focusRequester = remember { FocusRequester() }
    // Call is now always a deliberate tap — nothing here auto-focuses
    // because presence changed, only because there's something to
    // actually act on. Safe to request immediately: a stray hardware-Enter
    // key-up from a preceding screen is caught at the source by
    // MainActivity's own HardwareEnterKeyUpGuard.
    LaunchedEffect(canCall) { if (canCall) focusRequester.requestFocus() }
    // Purely informational, same as web's own identical dot — Call itself
    // stays unconditionally offered regardless of status (see this row's
    // own `canCall` param); tapping Call on an offline contact just
    // defers, and on a busy one resolves via the real busy-signal exchange
    // (see handle_peer_busy's own doc).
    val badge = when (status) {
        CallCoreBridge.PresenceStatus.BUSY -> GeneratedColor.colorStatusBusy to "Busy"
        CallCoreBridge.PresenceStatus.ONLINE -> GeneratedColor.colorStatusOk to "Online"
        CallCoreBridge.PresenceStatus.OFFLINE -> GeneratedColor.colorActionDangerBackground to "Offline"
    }
    val nameStyle = Type.contactName
    val displayName = name.ifBlank { "Unnamed contact" }
    // Three real table columns, not three loosely-flexed groups: this Row
    // fillMaxWidth()s to the shared width WaitingScreen's list Column
    // resolves via width(IntrinsicSize.Max), and column 1 uses
    // weight(fill = true) so it always expands to exactly fill what's left,
    // putting Call and Auto-answer/Delete at the same x on every row.
    Row(verticalAlignment = Alignment.CenterVertically, modifier = Modifier.fillMaxWidth()) {
        // Column 1: status dot + name. Top-aligned next to the name (like
        // iOS's own corner status chips) rather than baseline-aligned.
        Row(verticalAlignment = Alignment.Top, modifier = Modifier.weight(1f, fill = true).padding(end = Dimens.spacingContactRowGap)) {
            StatusDot(hue = badge.first, contentDescription = badge.second)
            Text(
                displayName,
                color = GeneratedColor.colorTextPrimary,
                style = nameStyle,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
                modifier = Modifier.weight(1f, fill = false),
            )
        }
        // Column 2: Call. Always composed, even when !canCall — a
        // conditionally-omitted button would shrink column 1's available
        // space and shift column 3 left, breaking the shared column
        // position the IntrinsicSize.Max trick depends on. enabled =
        // canCall already excludes a disabled Button from D-pad focus
        // traversal, and alpha = 0f hides it without removing it from
        // layout.
        TvButton(
            onClick = onCall,
            tint = TvButtonTint.Success,
            enabled = canCall,
            modifier = Modifier
                .focusRequester(focusRequester)
                .padding(start = Dimens.spacingContactRowGap, end = Dimens.spacingContactRowGap * 2)
                .alpha(if (canCall) 1f else 0f),
        ) { Icon(Icons.Filled.Call, contentDescription = "Call", modifier = Modifier.size(Dimens.dimension20)) }
        // Column 3: Auto-answer + Delete, right-aligned as a group.
        Row(verticalAlignment = Alignment.CenterVertically) {
            val switchInteractionSource = remember { MutableInteractionSource() }
            val switchFocused by switchInteractionSource.collectIsFocusedAsState()
            // Label stacked above the switch, brighter on focus (driven by
            // the Switch's own interactionSource) so the two read as one
            // small control.
            Column(
                horizontalAlignment = Alignment.CenterHorizontally,
                modifier = Modifier.padding(end = Dimens.dimension8),
            ) {
                Text(
                    "Auto-answer",
                    color = if (switchFocused) Color.White else GeneratedColor.colorTextDim,
                    // Real fontSize override, not Modifier.scale() — scale
                    // only shrinks what's painted, not the layout box
                    // reserved for it, leaving the same visual gap as
                    // before despite the smaller glyph.
                    style = Type.statusRow.copy(fontSize = Type.statusRow.fontSize * 0.85f),
                    modifier = Modifier.padding(bottom = Dimens.dimension4),
                )
                Switch(
                    checked = autoAnswer,
                    onCheckedChange = onToggleAutoAnswer,
                    // Every color slot spelled out — tv.material3's own
                    // theme-derived neutral grays read as mismatched
                    // against this screen's blue-tinted background.
                    colors = SwitchDefaults.colors(
                        checkedThumbColor = GeneratedColor.colorTextPrimary,
                        checkedTrackColor = GeneratedColor.colorStatusOk,
                        checkedBorderColor = GeneratedColor.colorStatusOk,
                        uncheckedThumbColor = GeneratedColor.colorTextDim,
                        uncheckedTrackColor = GeneratedColor.colorBackgroundSurfaceAlt,
                        uncheckedBorderColor = GeneratedColor.colorBorderDefault,
                    ),
                    interactionSource = switchInteractionSource,
                    modifier = Modifier
                        // scaledSize (not plain .scale()) so the Column
                        // this sits in actually reserves less space too —
                        // see that modifier's own doc.
                        .scaledSize(0.5f)
                        // The "Auto-answer" text label above is a separate
                        // node from this Switch's own — TalkBack landing
                        // directly on the Switch would otherwise announce a
                        // bare "on"/"off" with no indication of what it
                        // toggles or for whom.
                        .semantics { contentDescription = "Auto-answer for ${name.ifBlank { "this contact" }}" },
                )
            }
            DeleteIconButton(onClick = onDelete, contentDescription = "Delete ${name.ifBlank { "this contact" }}")
        }
    }
}

/**
 * A plain filled circle, not the status word spelled out in colored text —
 * the color alone was already the whole signal at this size. Mirrors
 * web's identical .contact-status-dot (styles.css).
 */
@Composable
private fun StatusDot(hue: Color, contentDescription: String, modifier: Modifier = Modifier) {
    Box(
        modifier = modifier
            .size(Dimens.dimension10)
            .background(hue, CircleShape)
            .semantics { this.contentDescription = contentDescription },
    )
}

/**
 * Delete, as a bare bin icon — grey when unfocused, brighter (not red)
 * once focused. A real tintable ImageVector, not a Unicode glyph: Android's
 * Noto Color Emoji renders the bin emoji as a fixed-color glyph, ignoring
 * a Text composable's own `color`, which broke the grey/bright focus swap
 * entirely.
 */
@Composable
private fun DeleteIconButton(onClick: () -> Unit, contentDescription: String, modifier: Modifier = Modifier) {
    val interactionSource = remember { MutableInteractionSource() }
    val focused by interactionSource.collectIsFocusedAsState()
    Box(
        modifier = modifier
            .size(Dimens.dimension36)
            .clickable(interactionSource = interactionSource, indication = null, onClick = onClick),
        contentAlignment = Alignment.Center,
    ) {
        Icon(
            DeleteBinIcon,
            contentDescription = contentDescription,
            tint = if (focused) Color.White else GeneratedColor.colorTextDim,
            // Touch/D-pad target stays dimension36 regardless — this only
            // shrinks the glyph, not what's actually focusable/clickable.
            modifier = Modifier.size(Dimens.dimension16),
        )
    }
}


/**
 * Shared implementation behind [RemoteVideoView] and [LocalPreviewView] —
 * identical `SurfaceViewRenderer` init/attach/detach/release lifecycle,
 * differing only in whether the preview needs mirroring and which pair of
 * `CameraAgentService` methods to attach/detach through. Kept as one
 * implementation so the `setZOrderMediaOverlay(true)` workaround below
 * can't silently drift if "fixed" in only one copy later.
 */
@Composable
private fun SurfaceVideoView(
    service: CameraAgentService?,
    ready: Boolean,
    mirror: Boolean,
    attach: CameraAgentService.(VideoSink) -> Unit,
    detach: CameraAgentService.() -> Unit,
    modifier: Modifier = Modifier,
) {
    val renderer = remember { mutableStateOf<SurfaceViewRenderer?>(null) }
    val initialized = remember { mutableStateOf(false) }
    AndroidView(
        modifier = modifier.fillMaxSize().onSizeChanged { size ->
            // Found live on real Portal hardware: this being the SAME
            // renderer instance across a layout-only resize (full-screen
            // while ringing -> a small corner box once connected, see
            // HomeScreen's own doc for why it's one instance now, not a
            // fresh one per screen) left the self-view invisible until
            // some LATER, unrelated layout change (cycling preview
            // position) happened to kick it. This hardware-overlay
            // SurfaceView's actual buffer geometry apparently isn't
            // always re-negotiated with the compositor by a plain
            // View layout pass alone — setFixedSize() explicitly forces
            // that renegotiation on every real size change, not just the
            // first. A no-op-ish redundant call on the size that already
            // applied automatically (e.g. plain corner-to-corner moves)
            // is harmless.
            if (initialized.value && size.width > 0 && size.height > 0) {
                runCatching { renderer.value?.holder?.setFixedSize(size.width, size.height) }
            }
        },
        factory = { ctx -> SurfaceViewRenderer(ctx).also { renderer.value = it } },
        // Runs exactly once, when this leaves composition for good —
        // unlike the DisposableEffect below, which re-fires on every
        // `ready` flip. SurfaceView is a hardware overlay outside
        // Compose's normal drawing; without an explicit release() its
        // Surface can keep compositing its last (black) frame on top of
        // whatever Compose draws next.
        onRelease = { view -> if (initialized.value) runCatching { view.release() } },
    )
    DisposableEffect(service, renderer.value, ready) {
        val view = renderer.value
        val eglContext = service?.eglContext
        if (view != null && eglContext != null && ready) {
            if (!initialized.value) {
                runCatching { view.init(eglContext, null) }
                view.setScalingType(RendererCommon.ScalingType.SCALE_ASPECT_FILL)
                if (mirror) view.setMirror(true)
                // Without this, a plain SurfaceView can claim a dedicated
                // hardware video plane on some TV hardware and blank the
                // normal 2D UI plane everywhere outside its own bounds —
                // exactly the "video shows, everything else goes black"
                // symptom hit on-device. This composites it normally
                // instead.
                view.setZOrderMediaOverlay(true)
                initialized.value = true
            }
            service.attach(view)
        }
        onDispose {
            if (view != null && initialized.value) service?.detach()
        }
    }
}

@Composable
private fun RemoteVideoView(service: CameraAgentService?, ready: Boolean, modifier: Modifier = Modifier) = SurfaceVideoView(
    service = service,
    ready = ready,
    mirror = false,
    attach = CameraAgentService::attachRemoteView,
    detach = CameraAgentService::detachRemoteView,
    modifier = modifier,
)

@Composable
private fun LocalPreviewView(service: CameraAgentService?, ready: Boolean, modifier: Modifier = Modifier) = SurfaceVideoView(
    service = service,
    ready = ready,
    mirror = true,
    attach = CameraAgentService::attachLocalPreview,
    detach = CameraAgentService::detachLocalPreview,
    modifier = modifier,
)
