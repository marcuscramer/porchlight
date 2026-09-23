//! Wraps the `spake2` crate's SPAKE2 (Ed25519Group) implementation,
//! specialized for this project's fully-symmetric pairing design: both
//! sides are completely interchangeable (there's no "shower" vs "scanner,"
//! no generator/enterer role — both people just type the same passphrase
//! into their own device), so this always uses `start_symmetric`, never
//! `start_a`/`start_b`.
//!
//! Owns the **whole** passphrase → verified-pairing pipeline, not just the
//! SPAKE2 math: trim + NFC-normalization, the rendezvous-tag hash, the
//! canonical-transcript ordering, and the key-confirmation HMAC + its
//! constant-time verification all live here too — the trim/normalize order
//! and the canonical-transcript tie-break are this protocol's own bespoke
//! rules, not something "any standard library gets right," so there's
//! exactly one implementation of them, shared by both platforms.

use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use spake2::{Ed25519Group, Identity, Password, Spake2};
use subtle::ConstantTimeEq;
use unicode_normalization::UnicodeNormalization;

// No `android` or `wasm` binding module anymore — `call-core` is the sole
// platform-facing entry point on both Android and web now. It depends on
// this crate directly as a plain Rust library and re-exposes what it needs
// via its own JNI (`call-core/src/android.rs`) and WASM
// (`call-core/src/wasm.rs`) layers; `web-app/app.js` imports `call-core`'s
// WASM build directly, not this crate's.

/// Domain-separation constant mixed into both the rendezvous tag and the
/// SPAKE2 exchange itself, so this app's PAKE space never collides with an
/// unrelated app's use of the same passphrase. Not a per-party identity —
/// there's no "A" vs "B" left to bind to, since both sides are symmetric —
/// just a fixed, shared label both sides supply identically.
const IDENTITY: &[u8] = b"porchlight-pake-v1";

/// One pairing attempt's in-progress SPAKE2 state. Single-use: [`finish`]
/// consumes it, matching `spake2::Spake2::finish`'s own signature — model
/// this as single-use in both platform bindings too (Android: invalidate
/// the JNI handle after `finish`; WASM: let `wasm-bindgen` consume the
/// object as it already does for by-value `self` methods).
///
/// [`finish`]: PakeSession::finish
pub struct PakeSession {
    inner: Spake2<Ed25519Group>,
    /// This side's own outbound blinded message — kept so [`finish`] can
    /// build the canonical transcript internally without the caller having
    /// to hand it back in.
    ///
    /// [`finish`]: PakeSession::finish
    outbound: Vec<u8>,
}

impl PakeSession {
    /// Starts a new symmetric SPAKE2 exchange for the given passphrase.
    /// Trims and NFC-normalizes it first — two devices' IMEs/OSes can
    /// encode the same visible characters (an accented letter, in
    /// particular) as different Unicode byte sequences (NFC vs NFD), and a
    /// stray leading/trailing space from an on-screen keyboard is exactly
    /// the kind of thing a human can't see — either would otherwise make
    /// two people who typed "the same" phrase land on different rendezvous
    /// tags/SPAKE2 passwords and silently never find each other. Doing it
    /// once, here, means neither platform has its own copy of this step to
    /// get out of sync (see this module's own doc).
    ///
    /// Returns the session, the rendezvous tag (hex-encoded SHA-256 of the
    /// domain-separation identity concatenated with the normalized
    /// passphrase) to find a peer at, and this side's outbound blinded
    /// message to publish once a peer is found.
    pub fn start(raw_passphrase: &str) -> (Self, String, Vec<u8>) {
        let normalized: String = raw_passphrase.trim().nfc().collect();
        let passphrase_bytes = normalized.as_bytes();

        let mut hasher = Sha256::new();
        hasher.update(IDENTITY);
        hasher.update(passphrase_bytes);
        let rendezvous_tag = hex_encode(&hasher.finalize());

        let (inner, outbound) =
            Spake2::<Ed25519Group>::start_symmetric(&Password::new(passphrase_bytes), &Identity::new(IDENTITY));
        let outbound_for_caller = outbound.clone();
        (Self { inner, outbound }, rendezvous_tag, outbound_for_caller)
    }

    /// Completes the exchange given the peer's inbound blinded message, and
    /// — unlike the SPAKE2 exchange alone, which cannot detect a passphrase
    /// mismatch from this call by itself — actually proves the two sides
    /// typed the same phrase: internally orders this side's own outbound
    /// message and the peer's inbound one into a canonical transcript (raw
    /// byte comparison, not by role — there's no "A"/"B" left to order by
    /// in a fully symmetric protocol; the shorter message sorts first if
    /// one is a prefix of the other, matching `<[u8]>`'s own `Ord`), then
    /// returns `HMAC-SHA256(shared_secret, "confirm" || transcript)`,
    /// hex-encoded, as the tag to *send* to the peer. The caller compares
    /// the tag it *receives* against its own via [`verify_confirmation`] —
    /// a match there, not merely this call returning `Ok`, is what actually
    /// proves the exchange succeeded.
    pub fn finish(self, inbound: &[u8]) -> Result<String, String> {
        let outbound = self.outbound;
        let secret = self.inner.finish(inbound).map_err(|e| e.to_string())?;
        let transcript = canonical_transcript(&outbound, inbound);

        let mut mac = HmacSha256::new_from_slice(&secret).expect("HMAC accepts a key of any length");
        mac.update(b"confirm");
        mac.update(&transcript);
        Ok(hex_encode(&mac.finalize().into_bytes()))
    }
}

/// Constant-time comparison of two hex-encoded confirmation tags — the
/// actual proof a pairing succeeded (see [`PakeSession::finish`]'s doc). A
/// MAC comparison should be constant-time on principle: hex-decodes both
/// inputs before comparing raw bytes (so two differently-cased-but-equal
/// hex strings compare equal, and callers no longer need their own
/// `.toLowerCase()` convention to paper over that), and uses the `subtle`
/// crate rather than a hand-rolled per-platform compare — genuinely a place
/// where "obviously correct" and "actually constant-time" diverge,
/// especially under a JS engine's JIT/short-circuiting. Returns `false`
/// (not an error) for malformed hex or a length mismatch — either already
/// means "not a match," and there's nothing else a caller would do
/// differently.
pub fn verify_confirmation(local_confirmation_hex: &str, remote_confirmation_hex: &str) -> bool {
    match (hex_decode(local_confirmation_hex), hex_decode(remote_confirmation_hex)) {
        (Some(a), Some(b)) if a.len() == b.len() => bool::from(a.ct_eq(&b)),
        _ => false,
    }
}

type HmacSha256 = Hmac<Sha256>;

/// Orders two byte strings by raw lexicographic comparison, with the
/// shorter one sorting first when one is a prefix of the other — exactly
/// `<[u8] as Ord>::cmp`'s own semantics, which is why this is a one-liner.
fn canonical_transcript(a: &[u8], b: &[u8]) -> Vec<u8> {
    if a <= b { [a, b].concat() } else { [b, a].concat() }
}

/// `pub` — used internally for the rendezvous tag/confirmation hex above,
/// and re-exported by `call-core` (`crate::hex_encode` there) instead of
/// keeping a second, identical copy.
pub fn hex_encode(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::with_capacity(bytes.len() * 2), |mut acc, b| {
        let _ = write!(acc, "{b:02x}");
        acc
    })
}

/// `pub` — see [`hex_encode`]'s own doc for why.
pub fn hex_decode(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matching_passphrase_derives_matching_confirmation_tags() {
        let (a, tag_a, a_out) = PakeSession::start("correct horse battery staple");
        let (b, tag_b, b_out) = PakeSession::start("correct horse battery staple");
        assert_eq!(tag_a, tag_b, "same passphrase must derive the same rendezvous tag");
        let a_confirm = a.finish(&b_out).expect("a.finish");
        let b_confirm = b.finish(&a_out).expect("b.finish");
        assert!(verify_confirmation(&a_confirm, &b_confirm), "matching passphrases must produce verifiable confirmation tags");
    }

    #[test]
    fn mismatched_passphrase_fails_confirmation_not_finish() {
        // Documents the same gap the old crate-level doc called out: a
        // wrong passphrase does NOT surface as an Err from finish() itself
        // — verify_confirmation is what actually has to catch this.
        let (a, tag_a, a_out) = PakeSession::start("correct horse battery staple");
        let (b, tag_b, b_out) = PakeSession::start("a completely different phrase");
        assert_ne!(tag_a, tag_b, "different passphrases must derive different rendezvous tags");
        let a_confirm = a.finish(&b_out).expect("finish still succeeds on mismatch");
        let b_confirm = b.finish(&a_out).expect("finish still succeeds on mismatch");
        assert!(!verify_confirmation(&a_confirm, &b_confirm), "mismatched passphrases must fail confirmation");
    }

    #[test]
    fn repeated_exchanges_with_the_same_passphrase_derive_different_confirmation_tags() {
        // Confirms fresh ephemeral randomness is actually being drawn each
        // call — if this ever started failing, that would mean the
        // randomness source is broken or stubbed out somewhere.
        let (a1, _, a1_out) = PakeSession::start("same passphrase both times");
        let (b1, _, b1_out) = PakeSession::start("same passphrase both times");
        let confirm1 = a1.finish(&b1_out).expect("first exchange");
        let _ = b1.finish(&a1_out).expect("first exchange, other side");

        let (a2, _, a2_out) = PakeSession::start("same passphrase both times");
        let (b2, _, b2_out) = PakeSession::start("same passphrase both times");
        let confirm2 = a2.finish(&b2_out).expect("second exchange");
        let _ = b2.finish(&a2_out).expect("second exchange, other side");

        assert_ne!(confirm1, confirm2, "two independent exchanges with the same passphrase must not derive the same confirmation tag");
    }

    #[test]
    fn trim_and_nfc_normalize_are_applied_before_anything_crypto_relevant() {
        // The exact bug this crate now structurally closes: an incidental
        // leading/trailing space must not change the rendezvous tag or the
        // SPAKE2 password.
        let (_, tag_clean, _) = PakeSession::start("some shared phrase");
        let (_, tag_padded, _) = PakeSession::start("  some shared phrase  ");
        assert_eq!(tag_clean, tag_padded, "trimming must happen before the rendezvous tag is derived");

        // NFD vs NFC of the same visible text (é as e + combining acute,
        // vs the single precomposed character) must normalize identically.
        let nfc = "café";
        let nfd = "cafe\u{0301}";
        assert_ne!(nfc.as_bytes(), nfd.as_bytes(), "sanity check: these really are different byte sequences");
        let (_, tag_nfc, _) = PakeSession::start(nfc);
        let (_, tag_nfd, _) = PakeSession::start(nfd);
        assert_eq!(tag_nfc, tag_nfd, "NFC/NFD of the same visible passphrase must derive the same rendezvous tag");
    }

    #[test]
    fn verify_confirmation_is_case_insensitive_on_hex_and_rejects_malformed_input() {
        let (a, _, a_out) = PakeSession::start("case sensitivity check");
        let (b, _, b_out) = PakeSession::start("case sensitivity check");
        let a_confirm = a.finish(&b_out).expect("a.finish");
        let b_confirm = b.finish(&a_out).expect("b.finish");
        assert!(verify_confirmation(&a_confirm, &b_confirm.to_uppercase()));
        assert!(!verify_confirmation(&a_confirm, "not valid hex"));
        assert!(!verify_confirmation(&a_confirm, "ab")); // valid hex, wrong length
        assert!(!verify_confirmation(&a_confirm, ""));
    }
}
