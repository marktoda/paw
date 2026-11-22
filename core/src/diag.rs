//! Raw checks of how a real key behaves through the phone's card reader.
//!
//! The answers decide how the key session is built: whether the card can be
//! asked if the PIN is still verified, which encodings of a long command it
//! accepts for an SSH login, and what it says when nobody touches it. Each
//! check sends fixed commands and records only status words, lengths and
//! timings. The PIN is never part of a result.

use std::sync::Arc;
use std::time::Instant;

use crate::ApduTransport;

/// The outcome of one command sent to the key.
#[derive(Debug, Clone, uniffi::Record)]
pub struct CheckResult {
    pub name: String,
    /// The command in hex, with the PIN and signing payload replaced by words.
    pub command: String,
    /// Status word in hex, e.g. "9000", or "error: ..." if the reader failed.
    pub status: String,
    /// Bytes of response data, not counting the status word.
    pub response_len: u32,
    pub millis: u32,
}

/// The size of the data an SSH login for user `git` asks the card to sign.
const LOGIN_LEN: usize = 146;

struct Runner<'a> {
    transport: Arc<dyn ApduTransport>,
    on_touch_needed: &'a (dyn Fn() + Send + Sync),
    results: Vec<CheckResult>,
}

impl Runner<'_> {
    /// Sends one command, follows `61xx` "more data" answers, and records it.
    fn send(&mut self, name: &str, shown: &str, apdu: &[u8]) -> Option<u16> {
        let started = Instant::now();
        let mut total = 0usize;
        let mut next = apdu.to_vec();
        let outcome = loop {
            match self.transport.transmit(&next) {
                Err(e) => break Err(e),
                Ok(r) if r.len() < 2 => break Err("response shorter than a status word".into()),
                Ok(r) => {
                    total += r.len() - 2;
                    let sw = u16::from_be_bytes([r[r.len() - 2], r[r.len() - 1]]);
                    if sw >> 8 == 0x61 {
                        next = vec![0x00, 0xC0, 0x00, 0x00, 0x00];
                    } else {
                        break Ok(sw);
                    }
                }
            }
        };
        self.results.push(CheckResult {
            name: name.into(),
            command: shown.into(),
            status: match &outcome {
                Ok(sw) => format!("{sw:04X}"),
                Err(e) => format!("error: {e}"),
            },
            response_len: total as u32,
            millis: started.elapsed().as_millis() as u32,
        });
        outcome.ok()
    }

    fn select(&mut self) -> Option<u16> {
        self.send(
            "Select the OpenPGP application",
            "00 A4 04 00 06 D2 76 00 01 24 01",
            &[0x00, 0xA4, 0x04, 0x00, 0x06, 0xD2, 0x76, 0x00, 0x01, 0x24, 0x01],
        )
    }

    fn pin_state(&mut self, name: &str) -> Option<u16> {
        self.send(name, "00 20 00 82", &[0x00, 0x20, 0x00, 0x82])
    }

    fn verify_short(&mut self, pin: &str) -> Option<u16> {
        let mut apdu = vec![0x00, 0x20, 0x00, 0x82, pin.len() as u8];
        apdu.extend(pin.as_bytes());
        self.send("Verify the PIN, short length", "00 20 00 82 Lc <pin>", &apdu)
    }
}

fn login() -> Vec<u8> {
    (0..LOGIN_LEN).map(|i| i as u8).collect()
}

/// Checks that need the PIN and three touches: PIN state queries, both
/// encodings of VERIFY, and a login-sized signature sent three ways.
pub fn card_checks(
    transport: Arc<dyn ApduTransport>,
    pin: &str,
    on_touch_needed: &(dyn Fn() + Send + Sync),
) -> Vec<CheckResult> {
    let mut run = Runner { transport, on_touch_needed, results: vec![] };
    if pin.is_empty() || pin.len() > 127 {
        return run.results;
    }
    let payload = login();
    let len = LOGIN_LEN as u8;

    run.select();
    run.send("Read the card's data", "00 CA 00 6E 00", &[0x00, 0xCA, 0x00, 0x6E, 0x00]);
    run.send(
        "Read the authentication public key",
        "00 47 81 00 02 A4 00 00",
        &[0x00, 0x47, 0x81, 0x00, 0x02, 0xA4, 0x00, 0x00],
    );
    run.pin_state("Ask whether the PIN is verified, before");
    if run.verify_short(pin) != Some(0x9000) {
        // Stop here: a second attempt could use another try.
        return run.results;
    }
    run.pin_state("Ask whether the PIN is verified, after");

    let mut extended = vec![0x00, 0x20, 0x00, 0x82, 0x00, 0x00, pin.len() as u8];
    extended.extend(pin.as_bytes());
    run.send("Verify the PIN, extended length", "00 20 00 82 00 00 Lc <pin>", &extended);

    let mut short = vec![0x00, 0x88, 0x00, 0x00, len];
    short.extend(&payload);
    short.push(0x00);
    (run.on_touch_needed)();
    run.send("Sign a login, short length (touch)", "00 88 00 00 92 <146 bytes> 00", &short);

    let mut long = vec![0x00, 0x88, 0x00, 0x00, 0x00, 0x00, len];
    long.extend(&payload);
    long.extend([0x00, 0x00]);
    (run.on_touch_needed)();
    run.send(
        "Sign a login, extended length (touch)",
        "00 88 00 00 00 00 92 <146 bytes> 00 00",
        &long,
    );

    let (first, rest) = payload.split_at(100);
    let mut part = vec![0x10, 0x88, 0x00, 0x00, first.len() as u8];
    part.extend(first);
    if run.send("Sign a login, chained, first part", "10 88 00 00 64 <100 bytes>", &part) == Some(0x9000) {
        let mut last = vec![0x00, 0x88, 0x00, 0x00, rest.len() as u8];
        last.extend(rest);
        last.push(0x00);
        (run.on_touch_needed)();
        run.send("Sign a login, chained, last part (touch)", "00 88 00 00 2E <46 bytes> 00", &last);
    }
    run.pin_state("Ask whether the PIN is verified, at the end");
    run.results
}

/// Asks for one signature that nobody should touch for, to learn the status
/// word and the delay of a touch timeout, then what the PIN state is after.
pub fn card_touch_timeout_check(transport: Arc<dyn ApduTransport>, pin: &str) -> Vec<CheckResult> {
    let mut run = Runner { transport, on_touch_needed: &|| {}, results: vec![] };
    if pin.is_empty() || pin.len() > 127 {
        return run.results;
    }
    run.select();
    if run.verify_short(pin) != Some(0x9000) {
        return run.results;
    }
    let mut short = vec![0x00, 0x88, 0x00, 0x00, LOGIN_LEN as u8];
    short.extend(login());
    short.push(0x00);
    run.send("Sign a login with no touch", "00 88 00 00 92 <146 bytes> 00", &short);
    run.pin_state("Ask whether the PIN is verified, after the timeout");
    run.results
}
