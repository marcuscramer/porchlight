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

use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Nonce};
use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use spake2::{Ed25519Group, Identity, Password, Spake2};
use subtle::ConstantTimeEq;
use unicode_normalization::UnicodeNormalization;

// No `android` or `wasm` binding module anymore — `call-core` is the sole
// platform-facing entry point on both Android and web now. It depends on
// this crate directly as a plain Rust library and re-exposes what it needs
// via its own JNI (`call-core/src/android.rs`) and WASM
// (`call-core/src/wasm.rs`) layers; `web-app/app.js` imports `call-core`'s
// WASM build directly, not this crate's.

/// Domain-separation constant mixed into the SPAKE2 exchange itself, so this
/// app's PAKE space never collides with an unrelated app's use of the same
/// passphrase. Not a per-party identity —
/// there's no "A" vs "B" left to bind to, since both sides are symmetric —
/// just a fixed, shared label both sides supply identically.
const IDENTITY: &[u8] = b"porchlight-pake-v1";

/// Fixed salt for [`derive_rendezvous_tag`]. Not secret and not per-user — both
/// devices must derive the *same* tag from the same passphrase with nothing
/// but the passphrase to go on, so a random salt isn't possible. Its job is
/// domain separation (this app's tags never equal another app's hash of the
/// same phrase) and versioning: change the `v2` if the derivation ever
/// changes, since an old and a new client can then no longer find each other.
const RENDEZVOUS_SALT: &[u8] = b"porchlight-rendezvous-v3";

/// Argon2id cost for the rendezvous tag. The tag is published on public
/// relays, so anyone watching can test passphrase guesses against it offline;
/// these make each guess cost this much time and memory instead of a
/// nanosecond of SHA-256. Both devices pay it once per pairing attempt, so it
/// has to stay quick on a Portal TV's CPU and inside a browser's WASM —
/// measured, not guessed (see the commit that introduced it).
const ARGON2_MEMORY_KIB: u32 = 64 * 1024;
const ARGON2_ITERATIONS: u32 = 3;
const ARGON2_LANES: u32 = 1;

/// The rendezvous tag two devices find each other at: hex Argon2id of the
/// normalized passphrase. Deliberately slow — see [`ARGON2_MEMORY_KIB`].
///
/// This only raises the cost of guessing a weak phrase from the public tag; a
/// short or common phrase still falls to a determined attacker, so the app's
/// guidance to choose a long one stands.
fn derive_rendezvous_tag(normalized_passphrase: &[u8]) -> String {
    let params = Params::new(ARGON2_MEMORY_KIB, ARGON2_ITERATIONS, ARGON2_LANES, Some(32))
        .expect("the Argon2 parameters above are valid constants");
    let mut tag = [0u8; 32];
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(normalized_passphrase, RENDEZVOUS_SALT, &mut tag)
        .expect("Argon2 with fixed valid parameters and a >= 8 byte salt cannot fail");
    hex_encode(&tag)
}

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
    /// Returns the session, the rendezvous tag (see `derive_rendezvous_tag`)
    /// to find a peer at, and this side's outbound blinded message to
    /// publish once a peer is found. Takes a noticeable fraction of a second
    /// — the tag derivation is deliberately slow.
    pub fn start(raw_passphrase: &str) -> (Self, String, Vec<u8>) {
        let normalized: String = raw_passphrase.trim().nfc().collect();
        let passphrase_bytes = normalized.as_bytes();

        let rendezvous_tag = derive_rendezvous_tag(passphrase_bytes);

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
    pub fn finish(self, inbound: &[u8]) -> Result<PakeKeys, String> {
        let outbound = self.outbound;
        // Reflection: with a symmetric exchange nothing stops an attacker
        // who has never seen the passphrase from echoing this side's own
        // message back as "the peer's". The shared secret would then be one
        // only this side can compute, and echoing this side's own
        // confirmation back would verify, so the attacker would be accepted
        // as a peer. The same check python-spake2's symmetric mode makes.
        if inbound == outbound.as_slice() {
            return Err("the peer's message is this side's own message, reflected back".to_string());
        }
        let secret = self.inner.finish(inbound).map_err(|e| e.to_string())?;
        let transcript = canonical_transcript(&outbound, inbound);

        let mut keys = PakeKeys { secret, transcript, confirmation_hex: String::new() };
        let confirm_key = keys.derive(b"confirm");
        let mut mac = <HmacSha256 as Mac>::new_from_slice(&confirm_key).expect("HMAC accepts a key of any length");
        mac.update(b"confirm");
        mac.update(&keys.transcript);
        keys.confirmation_hex = hex_encode(&mac.finalize().into_bytes());
        Ok(keys)
    }
}

/// Salt for the HKDF that splits the SPAKE2 secret into independent keys.
/// Versioned with the rest of the pairing format: bump it with
/// [`RENDEZVOUS_SALT`] if anything in the derivation changes.
const KEY_SALT: &[u8] = b"porchlight-pake-v3";

/// A device name is sealed into a fixed-size plaintext (one length byte plus
/// the name's UTF-8 bytes, zero-padded) so the ciphertext's length doesn't
/// say how long the name is.
const NAME_PLAINTEXT_LEN: usize = 128;
const MAX_NAME_BYTES: usize = NAME_PLAINTEXT_LEN - 1;
const NONCE_LEN: usize = 12;
const TAG_LEN: usize = 16;

/// Hex length of a sealed name: nonce + padded plaintext + Poly1305 tag.
pub const SEALED_NAME_HEX_LEN: usize = (NONCE_LEN + NAME_PLAINTEXT_LEN + TAG_LEN) * 2;

/// What a completed SPAKE2 exchange yields: the proof both sides compare
/// ([`PakeKeys::confirmation_hex`]) and the means to exchange the device
/// name privately once that proof checks out.
///
/// The SPAKE2 secret is never used directly. It goes through HKDF so the
/// confirmation MAC and each side's name key are independent: the name key
/// also depends on *whose* name it is (the sender's public key), so a sealed
/// name from one side can't be replayed back to it as the other side's.
pub struct PakeKeys {
    secret: Vec<u8>,
    transcript: Vec<u8>,
    confirmation_hex: String,
}

impl PakeKeys {
    /// The tag to send the peer; compare theirs with [`verify_confirmation`].
    pub fn confirmation_hex(&self) -> &str {
        &self.confirmation_hex
    }

    fn derive(&self, info: &[u8]) -> [u8; 32] {
        let mut out = [0u8; 32];
        Hkdf::<Sha256>::new(Some(KEY_SALT), &self.secret)
            .expand(info, &mut out)
            .expect("32 bytes is a valid HKDF-SHA256 output length");
        out
    }

    /// The tag proving a decision ("accept" or "cancel") about this pairing was made by the side whose signing key
    /// is `sender_pubkey_hex`: an HMAC under a key only a side that typed the same phrase has, over the decision,
    /// the sender and the transcript. Bound to the sender so one side's tag can't be echoed back as the other's,
    /// and to the decision so an accept can't be passed off as a cancel.
    pub fn decision_tag_hex(&self, decision: &str, sender_pubkey_hex: &str) -> String {
        let mut mac = <HmacSha256 as Mac>::new_from_slice(&self.derive(b"decision")).expect("HMAC accepts a key of any length");
        mac.update(decision.as_bytes());
        mac.update(&[0]);
        mac.update(sender_pubkey_hex.as_bytes());
        mac.update(&[0]);
        mac.update(&self.transcript);
        hex_encode(&mac.finalize().into_bytes())
    }

    /// Whether `tag_hex` is what [`PakeKeys::decision_tag_hex`] gives for this decision and sender.
    pub fn verify_decision(&self, decision: &str, sender_pubkey_hex: &str, tag_hex: &str) -> bool {
        verify_confirmation(&self.decision_tag_hex(decision, sender_pubkey_hex), tag_hex)
    }

    fn name_cipher(&self, sender_pubkey_hex: &str) -> ChaCha20Poly1305 {
        let mut info = b"name".to_vec();
        info.extend_from_slice(sender_pubkey_hex.as_bytes());
        ChaCha20Poly1305::new(&self.derive(&info).into())
    }

    /// Encrypts `name` (truncated to fit, at a character boundary) for the
    /// peer. `own_pubkey_hex` is the key this side signs its messages with.
    /// Bound to this exchange via the transcript. Call it once and reuse the
    /// result, so republished messages are byte-identical.
    pub fn seal_name(&self, own_pubkey_hex: &str, name: &str) -> String {
        let mut end = name.len().min(MAX_NAME_BYTES);
        while !name.is_char_boundary(end) {
            end -= 1;
        }
        let mut plaintext = [0u8; NAME_PLAINTEXT_LEN];
        plaintext[0] = end as u8;
        plaintext[1..=end].copy_from_slice(&name.as_bytes()[..end]);

        let mut nonce = [0u8; NONCE_LEN];
        getrandom::getrandom(&mut nonce).expect("the OS random number generator is available");
        let ciphertext = self
            .name_cipher(own_pubkey_hex)
            .encrypt(Nonce::from_slice(&nonce), Payload { msg: &plaintext, aad: &self.transcript })
            .expect("encrypting a fixed-size plaintext cannot fail");
        hex_encode(&[nonce.as_slice(), ciphertext.as_slice()].concat())
    }

    /// Decrypts a name sealed by the peer whose signing key is
    /// `sender_pubkey_hex`. `None` for anything that isn't exactly what
    /// [`PakeKeys::seal_name`] produces for this exchange and sender.
    pub fn open_name(&self, sender_pubkey_hex: &str, sealed_hex: &str) -> Option<String> {
        if sealed_hex.len() != SEALED_NAME_HEX_LEN {
            return None;
        }
        let bytes = hex_decode(sealed_hex)?;
        let (nonce, ciphertext) = bytes.split_at(NONCE_LEN);
        let plaintext = self
            .name_cipher(sender_pubkey_hex)
            .decrypt(Nonce::from_slice(nonce), Payload { msg: ciphertext, aad: &self.transcript })
            .ok()?;
        let len = *plaintext.first()? as usize;
        if len > MAX_NAME_BYTES || plaintext.len() != NAME_PLAINTEXT_LEN {
            return None;
        }
        String::from_utf8(plaintext[1..=len].to_vec()).ok()
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
        assert!(verify_confirmation(a_confirm.confirmation_hex(), b_confirm.confirmation_hex()), "matching passphrases must produce verifiable confirmation tags");
    }

    #[test]
    fn a_reflected_message_is_rejected_by_finish() {
        let (a, _, a_out) = PakeSession::start("correct horse battery staple");
        assert!(a.finish(&a_out).is_err(), "echoing a side's own message back must not complete an exchange");
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
        assert!(!verify_confirmation(a_confirm.confirmation_hex(), b_confirm.confirmation_hex()), "mismatched passphrases must fail confirmation");
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

        assert_ne!(confirm1.confirmation_hex(), confirm2.confirmation_hex(), "two independent exchanges with the same passphrase must not derive the same confirmation tag");
    }

    /// Locks the Argon2id parameters and salt: both platforms (and every
    /// future version of this app) must derive exactly this tag for this
    /// phrase, or two devices on different versions silently stop finding
    /// each other. If this fails because you meant to change the cost or the
    /// salt, bump `RENDEZVOUS_SALT`'s version, update this vector, and note
    /// that old and new clients can no longer pair with each other. The value
    /// was cross-checked against an independent implementation (argon2-cffi,
    /// the C reference code), and the previous version's against x86_64 and
    /// aarch64 (a real Portal TV) as well.
    #[test]
    fn rendezvous_tag_matches_the_known_answer_vector() {
        let (_s, tag, _o) = PakeSession::start("tangerine ferris wheel october");
        assert_eq!(tag, "2f87eec758acbd2b7c11dd367ff316fe3ace2816063b3b4f18874bdd892b4370");
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
        let (a_confirm, b_confirm) = (a_confirm.confirmation_hex(), b_confirm.confirmation_hex());
        assert!(verify_confirmation(a_confirm, &b_confirm.to_uppercase()));
        assert!(!verify_confirmation(a_confirm, "not valid hex"));
        assert!(!verify_confirmation(a_confirm, "ab")); // valid hex, wrong length
        assert!(!verify_confirmation(a_confirm, ""));
    }

    /// Two sides that typed the same phrase, with their finished keys.
    fn matched_pair() -> (PakeKeys, PakeKeys) {
        let (a, _, a_out) = PakeSession::start("name sealing test phrase");
        let (b, _, b_out) = PakeSession::start("name sealing test phrase");
        (a.finish(&b_out).expect("a.finish"), b.finish(&a_out).expect("b.finish"))
    }

    const KEY_A: &str = "aaaa";
    const KEY_B: &str = "bbbb";

    #[test]
    fn a_sealed_name_round_trips_to_the_other_side() {
        let (a, b) = matched_pair();
        let sealed = a.seal_name(KEY_A, "Living Room \u{1F3E0}");
        assert_eq!(b.open_name(KEY_A, &sealed).as_deref(), Some("Living Room \u{1F3E0}"));
        let sealed_empty = b.seal_name(KEY_B, "");
        assert_eq!(a.open_name(KEY_B, &sealed_empty).as_deref(), Some(""));
    }

    #[test]
    fn sealed_names_are_a_fixed_length_whatever_the_name() {
        let (a, _) = matched_pair();
        for name in ["", "x", "Mom's laptop", &"y".repeat(500)] {
            assert_eq!(a.seal_name(KEY_A, name).len(), SEALED_NAME_HEX_LEN, "length must not depend on the name");
        }
    }

    #[test]
    fn an_over_long_name_is_cut_at_a_character_boundary() {
        let (a, b) = matched_pair();
        // 3-byte characters: 127 bytes can't end mid-character.
        let long = "\u{20AC}".repeat(60);
        let opened = b.open_name(KEY_A, &a.seal_name(KEY_A, &long)).expect("opens");
        assert_eq!(opened, "\u{20AC}".repeat(42));
    }

    #[test]
    fn a_sealed_name_is_not_opened_as_if_it_came_from_a_different_sender() {
        // The reflection defence for names: a side's own sealed name echoed
        // back (claiming to be from someone else) must not open.
        let (a, b) = matched_pair();
        let sealed = a.seal_name(KEY_A, "Alice");
        assert_eq!(b.open_name(KEY_B, &sealed), None, "sealed for sender A, must not open as sender B");
        assert_eq!(a.open_name(KEY_B, &sealed), None, "own sealed name echoed back as the peer's must not open");
    }

    #[test]
    fn a_sealed_name_from_one_exchange_does_not_open_in_another() {
        let (a1, _) = matched_pair();
        let (_, b2) = matched_pair();
        assert_eq!(b2.open_name(KEY_A, &a1.seal_name(KEY_A, "Alice")), None);
    }

    #[test]
    fn a_sealed_name_from_a_mismatched_phrase_does_not_open() {
        let (a, _, a_out) = PakeSession::start("phrase one");
        let (b, _, b_out) = PakeSession::start("phrase two");
        let a_keys = a.finish(&b_out).unwrap();
        let b_keys = b.finish(&a_out).unwrap();
        assert_eq!(b_keys.open_name(KEY_A, &a_keys.seal_name(KEY_A, "Alice")), None);
    }

    #[test]
    fn a_tampered_or_malformed_sealed_name_is_rejected() {
        let (a, b) = matched_pair();
        let sealed = a.seal_name(KEY_A, "Alice");
        let mut flipped = sealed.clone().into_bytes();
        let last = flipped.len() - 1;
        flipped[last] = if flipped[last] == b'0' { b'1' } else { b'0' };
        assert_eq!(b.open_name(KEY_A, &String::from_utf8(flipped).unwrap()), None, "a flipped bit must fail authentication");
        assert_eq!(b.open_name(KEY_A, &sealed[..sealed.len() - 2]), None, "truncated");
        assert_eq!(b.open_name(KEY_A, &format!("{sealed}00")), None, "extended");
        assert_eq!(b.open_name(KEY_A, ""), None);
        assert_eq!(b.open_name(KEY_A, &"zz".repeat(SEALED_NAME_HEX_LEN / 2)), None, "not hex");
    }

    #[test]
    fn sealing_the_same_name_twice_gives_different_ciphertexts() {
        let (a, _) = matched_pair();
        assert_ne!(a.seal_name(KEY_A, "Alice"), a.seal_name(KEY_A, "Alice"), "nonce must be fresh per call");
    }

    #[test]
    fn a_decision_tag_verifies_only_for_the_same_decision_sender_and_exchange() {
        let (a, b) = matched_pair();
        let accept = a.decision_tag_hex("accept", KEY_A);
        assert!(b.verify_decision("accept", KEY_A, &accept), "the peer verifies what this side sent");
        assert!(!b.verify_decision("cancel", KEY_A, &accept), "an accept is not a cancel");
        assert!(!b.verify_decision("accept", KEY_B, &accept), "bound to the sender");
        assert!(!a.verify_decision("accept", KEY_B, &accept), "an echo of this side's own tag is not the peer's");
        assert!(!b.verify_decision("accept", KEY_A, ""), "empty");
        assert!(!b.verify_decision("accept", KEY_A, "zz"), "not hex");
        let (a2, _) = matched_pair();
        assert!(!a2.verify_decision("accept", KEY_A, &accept), "bound to the exchange");
    }

    #[test]
    fn a_decision_tag_from_a_mismatched_phrase_does_not_verify() {
        let (a, _, a_out) = PakeSession::start("phrase one");
        let (b, _, b_out) = PakeSession::start("phrase two");
        let a_keys = a.finish(&b_out).unwrap();
        let b_keys = b.finish(&a_out).unwrap();
        assert!(!b_keys.verify_decision("accept", KEY_A, &a_keys.decision_tag_hex("accept", KEY_A)));
    }
}
