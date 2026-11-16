//! The `source_records.payload` format: `{"v": 1, "dto": {…}, "raw": …}`.
//!
//! Merging and replay read `dto`; `raw` is the source's own JSON, kept for
//! debugging and later re-decoding. The hash is SHA-256 over the serialized
//! payload, which is deterministic (struct fields in declaration order,
//! JSON object keys sorted).

use chrono::{DateTime, Utc};
use domain::EntityType;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sources::DTO_VERSION;
use sqlx::SqliteConnection;
use store::refs::{self, RecordChange, SourceRecord};

use super::ReconcileError;

#[derive(Serialize)]
struct PayloadOut<'a, T> {
    v: u32,
    dto: &'a T,
    raw: &'a serde_json::Value,
}

#[derive(Deserialize)]
struct PayloadIn<T> {
    v: u32,
    dto: T,
}

/// Serializes a payload and returns it with its hash.
pub fn encode<T: Serialize>(dto: &T, raw: &serde_json::Value) -> (String, String) {
    let payload = serde_json::to_string(&PayloadOut {
        v: DTO_VERSION,
        dto,
        raw,
    })
    .expect("DTOs always serialize");
    let digest = Sha256::digest(payload.as_bytes());
    let hash = digest.iter().map(|b| format!("{b:02x}")).collect();
    (payload, hash)
}

/// Reads the DTO back from a stored record.
pub fn decode<T: DeserializeOwned>(rec: &SourceRecord) -> Result<T, ReconcileError> {
    let bad = |msg: String| ReconcileError::Payload {
        source_name: rec.source.clone(),
        entity_type: rec.entity_type,
        external_id: rec.external_id.clone(),
        msg,
    };
    let p: PayloadIn<T> = serde_json::from_str(&rec.payload).map_err(|e| bad(e.to_string()))?;
    if p.v != DTO_VERSION {
        return Err(bad(format!("unsupported payload version {}", p.v)));
    }
    Ok(p.dto)
}

/// Stores the latest payload of one external entity.
pub async fn store<T: Serialize>(
    conn: &mut SqliteConnection,
    source: &str,
    entity_type: EntityType,
    external_id: &str,
    dto: &T,
    raw: &serde_json::Value,
    now: DateTime<Utc>,
) -> Result<RecordChange, ReconcileError> {
    let (payload, payload_hash) = encode(dto, raw);
    let rec = SourceRecord {
        source: source.to_owned(),
        entity_type,
        external_id: external_id.to_owned(),
        payload,
        payload_hash,
        fetched_at: now,
    };
    Ok(refs::upsert_record(conn, &rec).await?)
}

/// Loads and decodes the stored DTO of one external entity.
pub async fn load<T: DeserializeOwned>(
    conn: &mut SqliteConnection,
    source: &str,
    entity_type: EntityType,
    external_id: &str,
) -> Result<Option<T>, ReconcileError> {
    match refs::get_record(&mut *conn, source, entity_type, external_id).await? {
        Some(rec) => decode(&rec).map(Some),
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sources::SrcTeam;

    #[test]
    fn encode_is_deterministic_and_decodes() {
        let t = SrcTeam::named("t1", "Alpha");
        let raw = serde_json::json!({"z": 1, "a": [1, 2]});
        let (p1, h1) = encode(&t, &raw);
        let (p2, h2) = encode(&t, &raw);
        assert_eq!((p1.clone(), h1.clone()), (p2, h2));
        assert_eq!(h1.len(), 64);
        assert!(p1.starts_with(r#"{"v":1,"dto":{"#), "{p1}");

        let rec = SourceRecord {
            source: "alpha".into(),
            entity_type: EntityType::Team,
            external_id: "t1".into(),
            payload: p1,
            payload_hash: h1,
            fetched_at: Utc::now(),
        };
        assert_eq!(decode::<SrcTeam>(&rec).unwrap(), t);

        let (_, h3) = encode(&SrcTeam::named("t1", "Alpha2"), &raw);
        assert_ne!(rec.payload_hash, h3);
    }

    #[test]
    fn unknown_version_is_rejected() {
        let rec = SourceRecord {
            source: "alpha".into(),
            entity_type: EntityType::Team,
            external_id: "t1".into(),
            payload: r#"{"v":99,"dto":{"ext_id":"t1","name":"A"},"raw":null}"#.into(),
            payload_hash: "x".into(),
            fetched_at: Utc::now(),
        };
        let err = decode::<SrcTeam>(&rec).unwrap_err();
        assert!(err.to_string().contains("unsupported payload version 99"));
    }
}
