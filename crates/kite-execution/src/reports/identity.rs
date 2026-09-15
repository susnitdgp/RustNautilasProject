use nautilus_core::UUID4;
use nautilus_model::identifiers::{AccountId, InstrumentId};
use sha2::{Digest, Sha256};
pub fn account() -> AccountId {
    AccountId::from("KITE-SIM")
}
pub fn instrument() -> InstrumentId {
    InstrumentId::from("CRUDEOIL26SEPFUT.MCX")
}
/// Deterministic UUID-shaped native IDs, scoped by journal generation and event sequence.
pub fn report_id(generation: &str, kind: &str, sequence: usize) -> UUID4 {
    let digest = Sha256::digest(
        format!("kite-simulation-report-v1/{generation}/{kind}/{sequence}").as_bytes(),
    );
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 15) | 64;
    bytes[8] = (bytes[8] & 63) | 128;
    let h = bytes.iter().map(|b| format!("{b:02x}")).collect::<String>();
    UUID4::from(
        format!(
            "{}-{}-{}-{}-{}",
            &h[..8],
            &h[8..12],
            &h[12..16],
            &h[16..20],
            &h[20..]
        )
        .as_str(),
    )
}
