// Builds real Porchlight signaling events with the project's own wasm core (web-app/wasm), so the
// survey sends exactly the shapes (and sizes) the apps send. Throwaway keys only, never real pairings.
import { readFileSync } from 'node:fs';
import { fileURLToPath, pathToFileURL } from 'node:url';
import path from 'node:path';
import crypto from 'node:crypto';

const wasmDir = path.join(path.dirname(fileURLToPath(import.meta.url)), '../../../web-app/wasm');
const core = await import(pathToFileURL(path.join(wasmDir, 'call_core.js')).href);
core.initSync({ module: readFileSync(path.join(wasmDir, 'call_core_bg.wasm')) });

export const WRAP_KIND = 20336;
export const BOOTSTRAP_KIND = 20331;

/** A fresh secp256k1 key pair as hex: secret key and x-only public key (the Nostr form). */
export function newKeys() {
  const e = crypto.createECDH('secp256k1');
  e.generateKeys();
  return { sk: e.getPrivateKey('hex').padStart(64, '0'), pk: e.getPublicKey('hex', 'compressed').slice(2) };
}

/** A plausible audio+video SDP of about `targetBytes` characters (the real one is ~7.4 KB in Chrome). */
export function makeSdp(targetBytes = 7400) {
  const hex = (n) => crypto.randomBytes(n).toString('hex');
  const fp = Array.from(crypto.randomBytes(32), (b) => b.toString(16).padStart(2, '0').toUpperCase()).join(':');
  const lines = [
    'v=0', `o=- ${Date.now()} 2 IN IP4 127.0.0.1`, 's=-', 't=0 0', 'a=group:BUNDLE 0 1', 'a=extmap-allow-mixed', 'a=msid-semantic: WMS survey',
    'm=audio 9 UDP/TLS/RTP/SAVPF 111 63 9 0 8 13 110 126', 'c=IN IP4 0.0.0.0', 'a=rtcp:9 IN IP4 0.0.0.0',
    `a=ice-ufrag:${hex(2)}`, `a=ice-pwd:${hex(12)}`, 'a=ice-options:trickle', `a=fingerprint:sha-256 ${fp}`, 'a=setup:actpass', 'a=mid:0',
    'a=extmap:1 urn:ietf:params:rtp-hdrext:ssrc-audio-level', 'a=extmap:2 http://www.webrtc.org/experiments/rtp-hdrext/abs-send-time',
    'a=sendrecv', 'a=msid:survey a0', 'a=rtcp-mux', 'a=rtpmap:111 opus/48000/2', 'a=rtcp-fb:111 transport-cc', 'a=fmtp:111 minptime=10;useinbandfec=1',
    'a=rtpmap:63 red/48000/2', 'a=fmtp:63 111/111', 'a=rtpmap:9 G722/8000', 'a=rtpmap:0 PCMU/8000', 'a=rtpmap:8 PCMA/8000', 'a=rtpmap:13 CN/8000',
    'a=rtpmap:110 telephone-event/48000', 'a=rtpmap:126 telephone-event/8000', `a=ssrc:${Math.floor(Math.random() * 1e9)} cname:${hex(8)}`,
    'm=video 9 UDP/TLS/RTP/SAVPF 96 97 98 99 100 101 102 103 104 105', 'c=IN IP4 0.0.0.0', 'a=rtcp:9 IN IP4 0.0.0.0',
    `a=ice-ufrag:${hex(2)}`, `a=ice-pwd:${hex(12)}`, 'a=ice-options:trickle', `a=fingerprint:sha-256 ${fp}`, 'a=setup:actpass', 'a=mid:1',
    'a=extmap:14 urn:ietf:params:rtp-hdrext:toffset', 'a=extmap:2 http://www.webrtc.org/experiments/rtp-hdrext/abs-send-time',
    'a=extmap:13 urn:3gpp:video-orientation', 'a=extmap:3 http://www.ietf.org/id/draft-holmer-rmcat-transport-wide-cc-extensions-01',
    'a=sendrecv', 'a=msid:survey v0', 'a=rtcp-mux', 'a=rtcp-rsize',
  ];
  const codecs = [['96', 'VP8/90000'], ['98', 'VP9/90000'], ['100', 'H264/90000'], ['102', 'H264/90000'], ['104', 'AV1/90000']];
  for (const [pt, name] of codecs) {
    lines.push(`a=rtpmap:${pt} ${name}`, `a=rtcp-fb:${pt} goog-remb`, `a=rtcp-fb:${pt} transport-cc`, `a=rtcp-fb:${pt} ccm fir`, `a=rtcp-fb:${pt} nack`, `a=rtcp-fb:${pt} nack pli`);
    lines.push(`a=fmtp:${pt} level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=42e01f`);
    lines.push(`a=rtpmap:${Number(pt) + 1} rtx/90000`, `a=fmtp:${Number(pt) + 1} apt=${pt}`);
  }
  let sdp = lines.join('\r\n') + '\r\n';
  let i = 0;
  while (sdp.length < targetBytes) sdp += `a=x-survey-pad:${i++}:${hex(24)}\r\n`;
  return sdp;
}

const callId = () => crypto.randomBytes(4).toString('hex');

/** A gift-wrapped heartbeat, as sent every 25 s per contact (~1.4 KB on the wire). */
export function heartbeatEvent(sender, recipientPk) {
  return JSON.parse(core.buildWrappedEvent(sender.sk, recipientPk, core.buildHeartbeatPayload('relay-survey', false)));
}

/** A gift-wrapped offer whose SDP is `sdpBytes` long (~2.3x that on the wire; a real offer is ~17 KB). */
export function offerEvent(sender, recipientPk, sdpBytes = 7400) {
  return JSON.parse(core.buildWrappedEvent(sender.sk, recipientPk, core.buildOfferPayload(makeSdp(sdpBytes), callId())));
}

/** A pairing-phase bootstrap event (kind 20331, `d`-tagged with the rendezvous tag). */
export function bootstrapEvent(sender, tag) {
  const payload = JSON.stringify({ type: 'pake1', msg: '02' + crypto.randomBytes(32).toString('hex') });
  return JSON.parse(core.buildBootstrapEvent(sender.sk, tag, null, payload));
}

/** Deterministic key pair from a label, so several processes can agree on each other's keys without sharing files. */
export function keysFrom(label) {
  const sk = crypto.createHash('sha256').update(label).digest('hex');
  const e = crypto.createECDH('secp256k1');
  e.setPrivateKey(sk, 'hex');
  return { sk, pk: e.getPublicKey('hex', 'compressed').slice(2) };
}

/** On-the-wire size of a real gift-wrapped message, to size the stand-ins below like the real thing. */
export function wrappedBytes(payloadJson) {
  const a = newKeys(), b = newKeys();
  return JSON.stringify(JSON.parse(core.buildWrappedEvent(a.sk, b.pk, payloadJson))).length;
}

/** A signed event whose content is plain JSON (the link test has no way to unwrap real gift wraps), padded so the
 * whole event is about `totalBytes` long. Kind 20331 carries a `p` tag when a target is given, so it can be
 * subscribed to by recipient exactly like the app's wraps. */
export function plainEvent(sender, targetPk, obj, totalBytes) {
  const build = (pad) => JSON.parse(core.buildBootstrapEvent(sender.sk, 'linktest', targetPk, JSON.stringify({ ...obj, pad })));
  const base = JSON.stringify(build('')).length;
  return build('x'.repeat(Math.max(0, totalBytes - base)));
}
