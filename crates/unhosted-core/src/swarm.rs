//! Content-addressed model distribution — manifest + chunk verification.
//!
//! This is the first slice of ADR-0014 (swarm model distribution): the
//! pure, peer-free core. It defines how a GGUF file is identified and
//! verified by content, so that bytes can later be fetched from any
//! source — a LAN peer, a trusted peer, or the HTTPS origin — and
//! trusted by the math rather than by the source.
//!
//! What lives here:
//!
//! - [`ModelManifest`]: the model's whole-file digest plus a per-chunk
//!   SHA-256 list. This is the Merkle-list (BitTorrent-v1-style) record
//!   that lets a node accept chunk *i* from one source and chunk *j*
//!   from another without trusting either.
//! - Hashing helpers: whole-file and per-chunk SHA-256, rendered as
//!   lowercase hex to match the `sha256:<hex>` digest convention.
//! - Verification: a single chunk against its expected hash, and a
//!   reassembled file against the whole-file digest.
//!
//! What does NOT live here (later slices of ADR-0014):
//!
//! - The peer wire protocol (`HaveManifest`/`GetChunk`/`ListModels`).
//! - Origin ranged fetch and source selection — that's in
//!   `model_manager.rs`, which calls into these functions.
//!
//! Everything in this module is pure (no I/O, no async) so it can be
//! unit-tested against synthetic byte buffers without a network or a
//! filesystem.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Fixed chunk size: 4 MiB. Balances manifest size against per-request
/// overhead. ADR-0014 flags this as a measure-before-committing value;
/// it's a single constant so a later slice can tune it (or make it
/// size-dependent for 70B-class files) in one place.
pub const CHUNK_SIZE: usize = 4 * 1024 * 1024;

/// The `sha256:` prefix every rendered digest carries. Keeping the
/// algorithm in the string means a future migration to a different hash
/// is unambiguous rather than a silent reinterpretation of bare hex.
pub const DIGEST_PREFIX: &str = "sha256:";

/// A model's content-addressed manifest. Fetched whole (it's tiny — a
/// 7 GB model is ~1750 chunk hashes, ~56 KB) before any chunk transfer.
///
/// The manifest is itself verifiable: recomputing every chunk hash from
/// the reassembled bytes and the whole-file digest must both match. A
/// peer cannot hand you a lying manifest that survives a completed
/// download, because the final whole-file check is independent of the
/// chunk list.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelManifest {
    /// `sha256:<hex>` of the entire file. The swap key: any bytes whose
    /// whole-file SHA-256 matches are a valid copy of this model.
    pub digest: String,
    /// Total file size in bytes. Lets a downloader size the progress
    /// bar and the chunk count before fetching any data.
    pub size_bytes: u64,
    /// Chunk size these hashes were computed at. Stored (not assumed)
    /// so a manifest produced under a different `CHUNK_SIZE` still
    /// verifies against the bytes it describes.
    pub chunk_size: u32,
    /// SHA-256 of each chunk, in file order. The last chunk may be
    /// shorter than `chunk_size`.
    pub chunks: Vec<[u8; 32]>,
}

impl ModelManifest {
    /// Build a manifest by hashing `data` in `CHUNK_SIZE` pieces. Used
    /// by the origin-seed path: when no peer has a manifest, the first
    /// puller synthesizes one from the bytes it fetched (ADR-0014
    /// "manifest origin for custom URLs", option a).
    pub fn from_bytes(data: &[u8]) -> Self {
        let chunks: Vec<[u8; 32]> = data.chunks(CHUNK_SIZE).map(sha256_chunk).collect();
        ModelManifest {
            digest: format_digest(&sha256_bytes(data)),
            size_bytes: data.len() as u64,
            chunk_size: CHUNK_SIZE as u32,
            chunks,
        }
    }

    /// Number of chunks this manifest describes.
    pub fn chunk_count(&self) -> usize {
        self.chunks.len()
    }

    /// Verify a single chunk's bytes against the expected hash at
    /// `index`. Returns `false` for an out-of-range index rather than
    /// panicking — a peer could send a bogus index and we must not
    /// crash on it.
    pub fn verify_chunk(&self, index: usize, bytes: &[u8]) -> bool {
        match self.chunks.get(index) {
            Some(expected) => &sha256_chunk(bytes) == expected,
            None => false,
        }
    }

    /// Verify a reassembled file against the whole-file digest. This is
    /// the final, source-independent check: even a manifest whose chunk
    /// hashes all matched can't get past a wrong whole-file digest.
    pub fn verify_whole(&self, data: &[u8]) -> bool {
        if data.len() as u64 != self.size_bytes {
            return false;
        }
        format_digest(&sha256_bytes(data)) == self.digest
    }
}

/// SHA-256 of a whole byte slice, raw 32 bytes.
pub fn sha256_bytes(data: &[u8]) -> [u8; 32] {
    Sha256::digest(data).into()
}

/// SHA-256 of one chunk, raw 32 bytes. Same computation as
/// [`sha256_bytes`]; named separately so call sites read as
/// "hashing a chunk" vs. "hashing the file".
fn sha256_chunk(chunk: &[u8]) -> [u8; 32] {
    Sha256::digest(chunk).into()
}

/// Render a raw 32-byte digest as `sha256:<64 lowercase hex>`.
pub fn format_digest(raw: &[u8; 32]) -> String {
    let mut s = String::with_capacity(DIGEST_PREFIX.len() + 64);
    s.push_str(DIGEST_PREFIX);
    for b in raw {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// True when `digest` is a well-formed `sha256:<64 hex>` string. Used
/// to validate catalog constants and any digest received from a peer
/// before it's trusted as a lookup key.
pub fn is_valid_digest(digest: &str) -> bool {
    let Some(hex) = digest.strip_prefix(DIGEST_PREFIX) else {
        return false;
    };
    hex.len() == 64
        && hex
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

// ─── peer chunk protocol ─────────────────────────────────────────────────
//
// ADR-0014 slice 3: three request/response shapes that ride the existing
// QUIC `PeerEndpoint` (transport.rs), dispatched by the same JSON-header
// framing `lib.rs`'s `quic_inbound_handler` already uses for "run":
//
//   <header-line-json>\n  e.g. {"kind":"have_manifest","version":0}
//   <body-json>            the request struct below
//
// Responses are JSON too, except `GetChunk`, whose response is a tiny
// JSON header line followed by the raw chunk bytes — JSON-encoding 4 MiB
// of binary would balloon it ~33% for no benefit. The server hashes the
// chunk before sending; the client re-hashes on receipt and verifies
// against the manifest, so neither side trusts the other's bytes.
//
// The message *types* and the disk-backed serving logic live here so
// they're unit-testable without a network; the quinn stream plumbing
// stays in lib.rs next to the existing "run" handler.

/// Stream-kind tags. Match the `kind` field in the QUIC header line.
pub mod kind {
    /// "do you have this model? send me its manifest." → `Manifest` | not-found.
    pub const HAVE_MANIFEST: &str = "have_manifest";
    /// "send me chunk N of this model." → raw bytes | not-found.
    pub const GET_CHUNK: &str = "get_chunk";
    /// "what models can you seed?" → `ListModelsResponse`.
    pub const LIST_MODELS: &str = "list_models";
}

/// `HaveManifest` request body: which model, by content digest.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HaveManifestRequest {
    pub digest: String,
}

/// `GetChunk` request body: which model + which chunk index.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GetChunkRequest {
    pub digest: String,
    pub index: u32,
}

/// One entry in a peer's seedable-model list.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SeedableModel {
    pub digest: String,
    pub file: String,
    pub size_bytes: u64,
}

/// `ListModels` response body.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ListModelsResponse {
    pub models: Vec<SeedableModel>,
}

/// JSON header line for a `GetChunk` *response* that carries bytes.
/// `found: false` means the peer doesn't have that chunk; no bytes
/// follow. `found: true` is followed by exactly `len` raw bytes.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChunkResponseHeader {
    pub found: bool,
    #[serde(default)]
    pub index: u32,
    #[serde(default)]
    pub len: u32,
}

/// Read-side of source-selection: given several peers' `ListModels`
/// answers, which peers can seed `digest`? Pure so the selection policy
/// (LAN before trusted before origin) is testable without a network.
pub fn peers_with_digest<'a>(
    catalogs: &'a [(String, ListModelsResponse)],
    digest: &str,
) -> Vec<&'a str> {
    catalogs
        .iter()
        .filter(|(_, resp)| resp.models.iter().any(|m| m.digest == digest))
        .map(|(name, _)| name.as_str())
        .collect()
}

// ─── disk-backed serving ─────────────────────────────────────────────────
//
// The server side of the protocol: answer manifest / chunk / list
// requests from `*.gguf` files already in the models dir. A manifest is
// computed on demand by hashing the file; for the file sizes involved a
// per-request full hash is wasteful, so callers should cache — but the
// pure functions here don't assume a cache, keeping them simple and
// testable. Caching is a lib.rs concern (it owns the long-lived state).

/// Build a [`ModelManifest`] for a file on disk, streaming it one
/// `CHUNK_SIZE` block at a time. Computes the per-chunk hashes and the
/// whole-file digest incrementally so peak memory is one chunk (4 MiB),
/// not the whole file — a 70B GGUF must not try to allocate ~40 GB just
/// to be hashed. The caller is expected to cache the result keyed by
/// path+mtime, since this still reads every byte off disk.
pub fn manifest_for_file(path: &std::path::Path) -> std::io::Result<ModelManifest> {
    use std::io::Read;
    let mut f = std::fs::File::open(path)?;
    let mut whole = Sha256::new();
    let mut chunks: Vec<[u8; 32]> = Vec::new();
    let mut size_bytes: u64 = 0;
    let mut buf = vec![0u8; CHUNK_SIZE];

    loop {
        // Fill a full chunk (or hit EOF). `read` may return short reads
        // mid-file, so loop until the buffer is full or the file ends —
        // otherwise chunk boundaries wouldn't line up with the readers
        // in `read_chunk` / the download path.
        let mut filled = 0;
        while filled < CHUNK_SIZE {
            let n = f.read(&mut buf[filled..])?;
            if n == 0 {
                break;
            }
            filled += n;
        }
        if filled == 0 {
            break; // clean EOF on a chunk boundary
        }
        let chunk = &buf[..filled];
        whole.update(chunk);
        chunks.push(Sha256::digest(chunk).into());
        size_bytes += filled as u64;
        if filled < CHUNK_SIZE {
            break; // short final chunk → EOF
        }
    }

    let digest_raw: [u8; 32] = whole.finalize().into();
    Ok(ModelManifest {
        digest: format_digest(&digest_raw),
        size_bytes,
        chunk_size: CHUNK_SIZE as u32,
        chunks,
    })
}

/// Read one `CHUNK_SIZE` chunk (`index`) from a file. Returns `Ok(None)`
/// when the index is past end-of-file. The bytes are returned raw; the
/// caller hashes + frames them.
pub fn read_chunk(path: &std::path::Path, index: u32) -> std::io::Result<Option<Vec<u8>>> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(path)?;
    let len = f.metadata()?.len();
    let start = index as u64 * CHUNK_SIZE as u64;
    if start >= len {
        return Ok(None);
    }
    let want = ((len - start) as usize).min(CHUNK_SIZE);
    f.seek(SeekFrom::Start(start))?;
    let mut buf = vec![0u8; want];
    f.read_exact(&mut buf)?;
    Ok(Some(buf))
}

/// List `*.gguf` files in `dir`. Kept here (rather than reusing
/// `model_manager::scan_models`) so `swarm` has no dependency on
/// `model_manager` — the dependency already runs the other way.
fn gguf_files_in(dir: &std::path::Path) -> Vec<(String, u64)> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            if !name.to_ascii_lowercase().ends_with(".gguf") {
                return None;
            }
            let meta = e.metadata().ok()?;
            if !meta.is_file() {
                return None;
            }
            Some((name, meta.len()))
        })
        .collect()
}

/// Enumerate the models in `dir` that this node can seed, each keyed by
/// its content digest. Hashes every file — callers should treat this as
/// expensive and not call it on a hot path. Used by the `list_models`
/// peer handler and the `seed-status` CLI command.
pub fn seedable_models_in(dir: &std::path::Path) -> Vec<SeedableModel> {
    gguf_files_in(dir)
        .into_iter()
        .filter_map(|(file, size_bytes)| {
            let path = dir.join(&file);
            let manifest = manifest_for_file(&path).ok()?;
            Some(SeedableModel {
                digest: manifest.digest,
                file,
                size_bytes,
            })
        })
        .collect()
}

/// Find the file in `dir` whose content matches `digest`, returning its
/// path + manifest. Hashes candidates until one matches; expensive, same
/// caveat as [`seedable_models_in`].
pub fn find_model_by_digest_in(
    dir: &std::path::Path,
    digest: &str,
) -> Option<(std::path::PathBuf, ModelManifest)> {
    for (file, _) in gguf_files_in(dir) {
        let path = dir.join(&file);
        if let Ok(manifest) = manifest_for_file(&path) {
            if manifest.digest == digest {
                return Some((path, manifest));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic pseudo-random bytes so tests cover multi-chunk
    /// files (and a non-aligned final chunk) without huge literals.
    fn synthetic(len: usize) -> Vec<u8> {
        let mut v = Vec::with_capacity(len);
        let mut x: u32 = 0x9e37_79b9;
        for _ in 0..len {
            x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            v.push((x >> 24) as u8);
        }
        v
    }

    #[test]
    fn format_digest_is_prefixed_lowercase_hex() {
        let raw = [0u8; 32];
        let d = format_digest(&raw);
        assert!(d.starts_with("sha256:"));
        assert_eq!(d.len(), "sha256:".len() + 64);
        assert!(is_valid_digest(&d));
    }

    #[test]
    fn is_valid_digest_rejects_malformed() {
        assert!(!is_valid_digest("9f86d0")); // no prefix
        assert!(!is_valid_digest("sha256:tooshort"));
        assert!(!is_valid_digest(
            "md5:0000000000000000000000000000000000000000000000000000000000000000"
        )); // wrong algo
            // uppercase hex is rejected so the lookup key is canonical
        assert!(!is_valid_digest(
            "sha256:ABCDEF0000000000000000000000000000000000000000000000000000000000"
        ));
        assert!(is_valid_digest(
            "sha256:abcdef0000000000000000000000000000000000000000000000000000000000"
        ));
    }

    #[test]
    fn manifest_single_chunk_round_trips() {
        let data = synthetic(1024); // < CHUNK_SIZE → one chunk
        let m = ModelManifest::from_bytes(&data);
        assert_eq!(m.chunk_count(), 1);
        assert_eq!(m.size_bytes, 1024);
        assert!(m.verify_chunk(0, &data));
        assert!(m.verify_whole(&data));
    }

    #[test]
    fn manifest_multi_chunk_with_ragged_tail() {
        // 2.5 chunks: two full + a short final chunk.
        let len = CHUNK_SIZE * 2 + 7;
        let data = synthetic(len);
        let m = ModelManifest::from_bytes(&data);
        assert_eq!(m.chunk_count(), 3);
        assert_eq!(m.size_bytes as usize, len);

        // Each chunk verifies against the right slice.
        for (i, chunk) in data.chunks(CHUNK_SIZE).enumerate() {
            assert!(m.verify_chunk(i, chunk), "chunk {i} should verify");
        }
        assert!(m.verify_whole(&data));
    }

    #[test]
    fn verify_chunk_rejects_wrong_bytes_and_bad_index() {
        let data = synthetic(CHUNK_SIZE + 100);
        let m = ModelManifest::from_bytes(&data);
        // Right index, tampered bytes.
        let mut bad = data[..CHUNK_SIZE].to_vec();
        bad[0] ^= 0xff;
        assert!(!m.verify_chunk(0, &bad));
        // Out-of-range index must return false, not panic.
        assert!(!m.verify_chunk(99, &data[..10]));
    }

    #[test]
    fn verify_whole_rejects_wrong_size_and_tampered_content() {
        let data = synthetic(CHUNK_SIZE + 100);
        let m = ModelManifest::from_bytes(&data);
        // Truncated → size mismatch.
        assert!(!m.verify_whole(&data[..data.len() - 1]));
        // Same length, one flipped byte → digest mismatch.
        let mut tampered = data.clone();
        let last = tampered.len() - 1;
        tampered[last] ^= 0x01;
        assert!(!m.verify_whole(&tampered));
    }

    #[test]
    fn manifest_serde_round_trips() {
        let m = ModelManifest::from_bytes(&synthetic(CHUNK_SIZE + 1));
        let json = serde_json::to_string(&m).unwrap();
        let back: ModelManifest = serde_json::from_str(&json).unwrap();
        assert_eq!(m, back);
    }

    #[test]
    fn from_bytes_digest_matches_independent_sha256() {
        let data = synthetic(5000);
        let m = ModelManifest::from_bytes(&data);
        assert_eq!(m.digest, format_digest(&sha256_bytes(&data)));
    }

    fn temp_file(name: &str, data: &[u8]) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("unhosted-swarm-{}-{}", std::process::id(), name));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("m.gguf");
        std::fs::write(&path, data).unwrap();
        path
    }

    #[test]
    fn manifest_for_file_matches_from_bytes() {
        // The streaming on-disk manifest MUST be byte-identical to the
        // in-memory one, or a seeder and a downloader would disagree on
        // chunk hashes. Cover a ragged multi-chunk file.
        let data = synthetic(CHUNK_SIZE * 2 + 4097);
        let path = temp_file("streameq", &data);
        let streamed = manifest_for_file(&path).unwrap();
        let in_memory = ModelManifest::from_bytes(&data);
        assert_eq!(streamed, in_memory);
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    #[test]
    fn manifest_for_file_handles_exact_chunk_multiple() {
        // Edge case: file size is an exact multiple of CHUNK_SIZE, so
        // the loop hits a clean EOF on a boundary with no short tail.
        let data = synthetic(CHUNK_SIZE * 2);
        let path = temp_file("exactmult", &data);
        let m = manifest_for_file(&path).unwrap();
        assert_eq!(m.chunk_count(), 2);
        assert_eq!(m.size_bytes as usize, CHUNK_SIZE * 2);
        assert_eq!(m, ModelManifest::from_bytes(&data));
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    #[test]
    fn read_chunk_returns_each_chunk_then_none() {
        let data = synthetic(CHUNK_SIZE * 2 + 11);
        let path = temp_file("readchunk", &data);
        let manifest = manifest_for_file(&path).unwrap();
        assert_eq!(manifest.chunk_count(), 3);

        // Every served chunk verifies against the manifest the same file
        // produced — this is the round-trip the peer protocol relies on.
        for i in 0..3u32 {
            let chunk = read_chunk(&path, i).unwrap().expect("chunk present");
            assert!(manifest.verify_chunk(i as usize, &chunk), "chunk {i}");
        }
        // Past EOF → None, not an error.
        assert!(read_chunk(&path, 3).unwrap().is_none());

        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    #[test]
    fn served_chunks_reassemble_into_a_verified_file() {
        // Simulate the client: pull every chunk, concatenate, verify the
        // whole-file digest. This is the end-to-end property of slice 3
        // minus the actual network hop.
        let data = synthetic(CHUNK_SIZE + 4096);
        let path = temp_file("reassemble", &data);
        let manifest = manifest_for_file(&path).unwrap();

        let mut assembled = Vec::new();
        let mut i = 0u32;
        while let Some(chunk) = read_chunk(&path, i).unwrap() {
            assert!(manifest.verify_chunk(i as usize, &chunk));
            assembled.extend_from_slice(&chunk);
            i += 1;
        }
        assert!(manifest.verify_whole(&assembled));
        assert_eq!(assembled, data);

        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    #[test]
    fn peers_with_digest_filters_to_holders() {
        let want = format_digest(&[1u8; 32]);
        let other = format_digest(&[2u8; 32]);
        let has = ListModelsResponse {
            models: vec![SeedableModel {
                digest: want.clone(),
                file: "a.gguf".into(),
                size_bytes: 10,
            }],
        };
        let hasnt = ListModelsResponse {
            models: vec![SeedableModel {
                digest: other,
                file: "b.gguf".into(),
                size_bytes: 20,
            }],
        };
        let catalogs = vec![("lan-box".to_string(), has), ("other".to_string(), hasnt)];
        let found = peers_with_digest(&catalogs, &want);
        assert_eq!(found, vec!["lan-box"]);
    }

    #[test]
    fn protocol_bodies_serde_round_trip() {
        let h = HaveManifestRequest {
            digest: format_digest(&[7u8; 32]),
        };
        let back: HaveManifestRequest =
            serde_json::from_str(&serde_json::to_string(&h).unwrap()).unwrap();
        assert_eq!(back.digest, h.digest);

        let hdr = ChunkResponseHeader {
            found: true,
            index: 5,
            len: 4096,
        };
        let s = serde_json::to_string(&hdr).unwrap();
        let back: ChunkResponseHeader = serde_json::from_str(&s).unwrap();
        assert!(back.found && back.index == 5 && back.len == 4096);
    }
}
