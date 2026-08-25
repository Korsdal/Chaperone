// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 SerenIT ApS
// Copyright 2026 Prompted EV

//! Container-format identification by magic bytes.
//!
//! ## Why this exists
//!
//! A `chapr_read` of a compressed container is an active footgun, not a passive
//! limitation. The bytes come back base64, and a model handed base64 does not
//! reliably refuse: it recognises the container header and confabulates. That
//! produces a confident summary of a document nobody read, written back into a
//! coordinated file and stamped with the user's AD principal — which is the one
//! failure this project's whole audit story cannot absorb, because the trail
//! would be *correct* about who caused it and useless about what it says.
//!
//! ## Why by content, never by extension
//!
//! The share contains misnamed files, and local knowledge is authoritative
//! elsewhere in this codebase already ([`crate::mount`] resolves what a path
//! really is rather than believing its spelling). An extension is a claim by
//! whoever last renamed the file; the first bytes are the file itself.
//!
//! It also matters in the *other* direction. Whether a container refuses is
//! decided here rather than by UTF-8 validity, because those are not the same
//! test: an uncompressed PDF can be almost entirely ASCII and pass
//! `std::str::from_utf8`, and would then be served as "text" — the exact
//! confabulation case above, arriving through the door marked safe.
//!
//! ## Weak signatures are deliberately omitted
//!
//! Nothing here matches on fewer bytes than can distinguish a container from
//! prose. `BM` (bitmap) is two ASCII letters and would refuse a text file
//! beginning "BMW"; it is left out on purpose. A false refusal of a readable
//! file is a worse bug than a missed refusal of a binary one, because the
//! missed case still has [`crate::server::render_envelope`]'s base64 path and
//! the envelope's `encoding=base64` header behind it.
//!
//! ## [`classify_unrecognised`] chooses a *sentence*, never an outcome
//!
//! Bytes that are not a known container and not valid UTF-8 are refused either
//! way. What [`classify_unrecognised`] decides is only **how to describe them**,
//! because "a Danish text file in a Windows code page" and "an unrecognised
//! binary" need opposite messages: the first is a routine file needing a re-save,
//! the second is worth an administrator's attention. Calling the first one binary
//! made an agent report a phantom binary to a user about their own `.txt`
//! (I-015).
//!
//! **Do not promote this into a serve-versus-refuse decision.** Both arms refuse,
//! which is what keeps its thresholds harmless: a misclassification costs one
//! wrong sentence, never a file served as text that should not have been. The
//! `BM`-bitmap argument above applies with full force the moment that changes.
//!
//! Nor does it transcode. Chaperone coordinates files; converting encodings is
//! not its job, and it could not do it safely from here anyway — the write side
//! (`chapr-endpoint`'s `decode_content`) can only emit UTF-8, so a transcoded
//! read echoed back would silently rewrite the file in a different encoding and
//! record it as a deliberate edit.

/// A recognised container format that cannot usefully be read as text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Container {
    Pdf,
    /// OOXML: `.docx`, `.xlsx`, `.pptx`. A zip whose first entry is
    /// `[Content_Types].xml`.
    Ooxml,
    /// OpenDocument: `.odt`, `.ods`, `.odp`. A zip whose first entry is `mimetype`.
    OpenDocument,
    /// OLE2 / Compound File Binary: legacy `.doc`, `.xls`, `.ppt`, `.msg`.
    LegacyOffice,
    Zip,
    Gzip,
    Bzip2,
    Xz,
    SevenZip,
    Rar,
    Png,
    Jpeg,
    Gif,
    Tiff,
    WebP,
    Sqlite,
}

/// How a container should be talked about when refusing it. The advice differs
/// per class, and only the class matters — a `.docx` and a `.pdf` get the same
/// sentence because the same thing fixes both.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    /// A document. A text mirror is the answer.
    Document,
    /// An archive. Unpacking is not Chaperone's job.
    Archive,
    /// An image. There is no path from here to a model that reads it.
    Image,
    /// A structured data file. Not prose, and not a document.
    Database,
}

impl Container {
    /// The name to use when telling a model what it asked for.
    pub fn label(self) -> &'static str {
        match self {
            Container::Pdf => "a PDF document",
            Container::Ooxml => "an Office document (docx/xlsx/pptx)",
            Container::OpenDocument => "an OpenDocument file (odt/ods/odp)",
            Container::LegacyOffice => "a legacy Office document (doc/xls/ppt)",
            Container::Zip => "a zip archive",
            Container::Gzip => "a gzip archive",
            Container::Bzip2 => "a bzip2 archive",
            Container::Xz => "an xz archive",
            Container::SevenZip => "a 7-Zip archive",
            Container::Rar => "a RAR archive",
            Container::Png => "a PNG image",
            Container::Jpeg => "a JPEG image",
            Container::Gif => "a GIF image",
            Container::Tiff => "a TIFF image",
            Container::WebP => "a WebP image",
            Container::Sqlite => "a SQLite database",
        }
    }

    pub fn class(self) -> Class {
        match self {
            Container::Pdf
            | Container::Ooxml
            | Container::OpenDocument
            | Container::LegacyOffice => Class::Document,
            Container::Zip
            | Container::Gzip
            | Container::Bzip2
            | Container::Xz
            | Container::SevenZip
            | Container::Rar => Class::Archive,
            Container::Png
            | Container::Jpeg
            | Container::Gif
            | Container::Tiff
            | Container::WebP => Class::Image,
            Container::Sqlite => Class::Database,
        }
    }
}

/// How far into the file a PDF header is still accepted.
///
/// The spec puts `%PDF-` at offset 0, but real-world files carry preambles from
/// scanners and mail gateways. Bounded so this stays a header check and does not
/// become a search that finds the string inside a legitimate text document.
const PDF_HEADER_WINDOW: usize = 1024;

/// Identify a non-analysable container from its leading bytes.
///
/// `None` means "not a container this knows about" — which is not the same as
/// "text". The caller still has to decide what to do with unrecognised bytes
/// that are not valid UTF-8.
pub fn identify(bytes: &[u8]) -> Option<Container> {
    if bytes.len() < 4 {
        return None;
    }

    // Zip family first: three signatures, and the interesting cases live inside.
    if bytes.starts_with(b"PK\x03\x04")
        || bytes.starts_with(b"PK\x05\x06")
        || bytes.starts_with(b"PK\x07\x08")
    {
        return Some(zip_flavour(bytes));
    }

    if starts_with_pdf(bytes) {
        return Some(Container::Pdf);
    }

    // Long, unambiguous signatures.
    if bytes.starts_with(b"\xD0\xCF\x11\xE0\xA1\xB1\x1A\xE1") {
        return Some(Container::LegacyOffice);
    }
    if bytes.starts_with(b"\x89PNG\r\n\x1A\n") {
        return Some(Container::Png);
    }
    if bytes.starts_with(b"SQLite format 3\0") {
        return Some(Container::Sqlite);
    }
    if bytes.starts_with(b"\xFD7zXZ\x00") {
        return Some(Container::Xz);
    }
    if bytes.starts_with(b"7z\xBC\xAF\x27\x1C") {
        return Some(Container::SevenZip);
    }
    if bytes.starts_with(b"Rar!\x1A\x07") {
        return Some(Container::Rar);
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Some(Container::Gif);
    }
    // RIFF....WEBP — the form matters, the four size bytes in between do not.
    if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        return Some(Container::WebP);
    }
    if bytes.starts_with(b"\xFF\xD8\xFF") {
        return Some(Container::Jpeg);
    }
    if bytes.starts_with(b"II*\0") || bytes.starts_with(b"MM\0*") {
        return Some(Container::Tiff);
    }
    if bytes.starts_with(b"BZh") {
        return Some(Container::Bzip2);
    }
    // 0x1F 0x8B: 0x8B is a UTF-8 continuation byte with no lead, so this cannot
    // be text however short the match is.
    if bytes.starts_with(b"\x1F\x8B") {
        return Some(Container::Gzip);
    }

    None
}

/// `%PDF-` at the start, or inside a bounded preamble window.
///
/// The windowed case carries an extra condition: the file must also fail UTF-8.
/// Without it, a perfectly readable document *discussing* PDFs — "the `%PDF-`
/// header identifies…" — would be refused, and refusing readable text is the
/// worse error. A real PDF behind a preamble still has compressed streams, so it
/// fails UTF-8 and is caught; prose about PDFs is valid UTF-8 and is served.
///
/// At offset 0 no such condition applies: a file whose very first bytes are
/// `%PDF-` is a PDF, and treating it as text is how the confabulation case gets
/// in through the door marked safe.
fn starts_with_pdf(bytes: &[u8]) -> bool {
    const MAGIC: &[u8] = b"%PDF-";
    if bytes.starts_with(MAGIC) {
        return true;
    }
    if std::str::from_utf8(bytes).is_ok() {
        return false;
    }
    let window = &bytes[..bytes.len().min(PDF_HEADER_WINDOW)];
    window.windows(MAGIC.len()).any(|w| w == MAGIC)
}

/// Tell OOXML and OpenDocument from a plain zip by the first entry's name.
///
/// A zip local file header puts the filename length at offset 26 and the name
/// itself at offset 30. Both formats mandate a specific first entry, so this is
/// the format's own rule rather than a guess — and worth the twenty lines,
/// because "an Office document, read its text mirror" and "a zip archive, unpack
/// it elsewhere" are different instructions to the caller.
fn zip_flavour(bytes: &[u8]) -> Container {
    const NAME_LEN_OFFSET: usize = 26;
    const NAME_OFFSET: usize = 30;

    if bytes.len() < NAME_OFFSET {
        return Container::Zip;
    }
    let name_len =
        u16::from_le_bytes([bytes[NAME_LEN_OFFSET], bytes[NAME_LEN_OFFSET + 1]]) as usize;
    let end = match NAME_OFFSET.checked_add(name_len) {
        Some(e) if e <= bytes.len() => e,
        // Truncated or absurd: fall back rather than guess.
        _ => return Container::Zip,
    };
    match &bytes[NAME_OFFSET..end] {
        b"[Content_Types].xml" => Container::Ooxml,
        b"mimetype" => Container::OpenDocument,
        _ => Container::Zip,
    }
}

/// A byte-order mark: a *declaration* by whoever wrote the file, not a guess.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bom {
    Utf8,
    Utf16Le,
    Utf16Be,
    Utf32Le,
    Utf32Be,
}

impl Bom {
    pub fn label(self) -> &'static str {
        match self {
            Bom::Utf8 => "utf-8",
            Bom::Utf16Le => "utf-16le",
            Bom::Utf16Be => "utf-16be",
            Bom::Utf32Le => "utf-32le",
            Bom::Utf32Be => "utf-32be",
        }
    }

    /// The UTF-32 forms must be tested before the UTF-16 ones they contain:
    /// `FF FE 00 00` (UTF-32LE) starts with `FF FE` (UTF-16LE).
    fn detect(bytes: &[u8]) -> Option<Bom> {
        if bytes.starts_with(&[0xFF, 0xFE, 0x00, 0x00]) {
            return Some(Bom::Utf32Le);
        }
        if bytes.starts_with(&[0x00, 0x00, 0xFE, 0xFF]) {
            return Some(Bom::Utf32Be);
        }
        if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
            return Some(Bom::Utf8);
        }
        if bytes.starts_with(&[0xFF, 0xFE]) {
            return Some(Bom::Utf16Le);
        }
        if bytes.starts_with(&[0xFE, 0xFF]) {
            return Some(Bom::Utf16Be);
        }
        None
    }
}

/// What the bytes of a non-UTF-8 text file appear to be encoded in.
///
/// Deliberately coarse. Windows-1252 and ISO-8859-1 cannot be told apart from
/// the bytes alone — the difference lives in `0x80..=0x9F`, and a file may simply
/// not use it — so this names the *family* and leaves the exact code page to the
/// person who knows which tool wrote the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextEncoding {
    Utf16Le,
    Utf16Be,
    Utf32Le,
    Utf32Be,
    /// A single-byte code page: Windows-1252, ISO-8859-1 or a relative.
    SingleByte,
    /// A UTF-8 byte-order mark, but the bytes after it are not valid UTF-8. The
    /// file declares an encoding it is not in, so the fault is in whatever wrote
    /// it — worth saying separately, because "re-save as UTF-8" is the remedy for
    /// a file that never claimed to be UTF-8, and this one did.
    Utf8Mislabelled,
    /// Not valid UTF-8 and plausibly text, but nothing identifies the encoding.
    Unknown,
}

impl TextEncoding {
    /// Phrased to complete "the file is saved in {label} rather than UTF-8".
    pub fn label(self) -> &'static str {
        match self {
            TextEncoding::Utf16Le => "UTF-16, little-endian",
            TextEncoding::Utf16Be => "UTF-16, big-endian",
            TextEncoding::Utf32Le => "UTF-32, little-endian",
            TextEncoding::Utf32Be => "UTF-32, big-endian",
            TextEncoding::SingleByte => {
                "a single-byte code page (Windows-1252 or a relative, which cannot be told \
                 apart from the bytes alone)"
            }
            TextEncoding::Utf8Mislabelled => {
                "something other than the UTF-8 its byte-order mark claims"
            }
            TextEncoding::Unknown => "an encoding that cannot be identified from its bytes",
        }
    }

    /// The stable machine form, for a diagnostic's facts.
    pub fn code(self) -> &'static str {
        match self {
            TextEncoding::Utf16Le => "utf-16le",
            TextEncoding::Utf16Be => "utf-16be",
            TextEncoding::Utf32Le => "utf-32le",
            TextEncoding::Utf32Be => "utf-32be",
            TextEncoding::SingleByte => "single-byte-code-page",
            TextEncoding::Utf8Mislabelled => "utf-8-bom-but-not-utf-8",
            TextEncoding::Unknown => "unknown",
        }
    }
}

/// What an administrator needs to tell one cause from another, and nothing more.
///
/// Structural only, deliberately: no excerpt of the file's content. These facts
/// travel to the coordinator's diagnostics store, which the admin page renders
/// without an ACL check, and the existence leak documented in concept §13.2 is
/// not worth widening into a content leak to save somebody one glance at the
/// file.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextEvidence {
    pub looks_like: TextEncoding,
    pub bom: Option<Bom>,
    /// Offset of the first byte that is not valid UTF-8 — the caller's own
    /// `Utf8Error::valid_up_to`, which is the authority on it.
    pub first_invalid_offset: usize,
    pub first_invalid_byte: u8,
    /// Share of bytes at or above `0x80`. A few percent reads as prose in a code
    /// page; a compressed stream is around half.
    pub high_byte_ratio: f32,
}

/// Bytes that are neither a known container nor valid UTF-8.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Unrecognised {
    /// Text in some encoding other than UTF-8. Still refused — see the module doc.
    NonUtf8Text(TextEvidence),
    /// Not plausibly text at all.
    Binary,
}

/// A file of NULs is not UTF-16 with an empty high byte. Requiring some non-NUL
/// bytes, and requiring *those* to look like text, is what separates the two.
const MIN_UTF16_NUL_SHARE: f32 = 0.25;
/// Almost all NULs must share one parity for a UTF-16 claim to be honest.
const MIN_UTF16_PARITY_SHARE: f32 = 0.9;
/// Above this share of high bytes, "text in a code page" stops being credible.
const MAX_TEXT_HIGH_BYTE_RATIO: f32 = 0.30;
/// Control bytes outside this set are the strongest single binary signal: prose
/// in any code page does not contain them, and arbitrary bytes almost always do.
fn is_allowed_control(b: u8) -> bool {
    matches!(b, 0x09 | 0x0A | 0x0C | 0x0D)
}

fn is_stray_control(b: u8) -> bool {
    (b < 0x20 && !is_allowed_control(b)) || b == 0x7F
}

/// Describe bytes that [`identify`] did not recognise and that failed UTF-8.
///
/// `first_invalid_offset` is the caller's `Utf8Error::valid_up_to()`. Taking it as
/// an argument rather than recomputing it makes the precondition part of the
/// signature: there is no "what if it was valid UTF-8" branch to get wrong,
/// because a caller with valid UTF-8 has nothing to pass.
///
/// **This picks a message, not an outcome.** Both variants are refused.
pub fn classify_unrecognised(bytes: &[u8], first_invalid_offset: usize) -> Unrecognised {
    let len = bytes.len();
    let first_invalid_byte = bytes.get(first_invalid_offset).copied().unwrap_or(0);

    let mut high = 0usize;
    let mut nul_even = 0usize;
    let mut nul_odd = 0usize;
    let mut strays = 0usize;
    let mut non_nul = 0usize;
    for (i, &b) in bytes.iter().enumerate() {
        if b >= 0x80 {
            high += 1;
        }
        if b == 0x00 {
            if i % 2 == 0 {
                nul_even += 1;
            } else {
                nul_odd += 1;
            }
        } else {
            non_nul += 1;
            if is_stray_control(b) {
                strays += 1;
            }
        }
    }
    let ratio = if len == 0 { 0.0 } else { high as f32 / len as f32 };
    let evidence = |looks_like, bom| {
        Unrecognised::NonUtf8Text(TextEvidence {
            looks_like,
            bom,
            first_invalid_offset,
            first_invalid_byte,
            high_byte_ratio: ratio,
        })
    };

    // 1. A BOM is a declaration. Trust it over any amount of counting — including
    //    when it is a lie, which is itself the useful finding.
    if let Some(bom) = Bom::detect(bytes) {
        let enc = match bom {
            Bom::Utf8 => TextEncoding::Utf8Mislabelled,
            Bom::Utf16Le => TextEncoding::Utf16Le,
            Bom::Utf16Be => TextEncoding::Utf16Be,
            Bom::Utf32Le => TextEncoding::Utf32Le,
            Bom::Utf32Be => TextEncoding::Utf32Be,
        };
        return evidence(enc, Some(bom));
    }

    // 2. BOM-less UTF-16: what PowerShell and older Windows editors leave behind
    //    when the BOM is stripped. Latin text puts a NUL in every other byte, and
    //    which parity says which endianness.
    let nuls = nul_even + nul_odd;
    if nuls > 0 && non_nul > 0 {
        let nul_share = nuls as f32 / len as f32;
        let dominant = nul_even.max(nul_odd) as f32 / nuls as f32;
        let text_like = (strays as f32 / non_nul as f32) < 0.05;
        if nul_share >= MIN_UTF16_NUL_SHARE && dominant >= MIN_UTF16_PARITY_SHARE && text_like {
            // Latin text as UTF-16LE is [lo, 00] pairs, so its NULs land on odd
            // offsets; big-endian is [00, hi] and lands on even ones.
            let enc = if nul_odd >= nul_even {
                TextEncoding::Utf16Le
            } else {
                TextEncoding::Utf16Be
            };
            return evidence(enc, None);
        }
    }

    // 3. No NULs and no stray controls is what a single-byte code page looks
    //    like. The high-byte share separates prose from dense binary that
    //    happens to avoid the control range.
    if nuls == 0 && strays == 0 && len > 0 {
        let enc = if ratio < MAX_TEXT_HIGH_BYTE_RATIO {
            TextEncoding::SingleByte
        } else {
            TextEncoding::Unknown
        };
        return evidence(enc, None);
    }

    Unrecognised::Binary
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal zip local file header with a chosen first-entry name.
    fn zip_with_first_entry(name: &[u8]) -> Vec<u8> {
        let mut v = vec![0u8; 30];
        v[..4].copy_from_slice(b"PK\x03\x04");
        let len = u16::try_from(name.len()).unwrap().to_le_bytes();
        v[26] = len[0];
        v[27] = len[1];
        v.extend_from_slice(name);
        v
    }

    #[test]
    fn identifies_a_pdf_at_offset_zero() {
        assert_eq!(identify(b"%PDF-1.7\n1 0 obj"), Some(Container::Pdf));
    }

    /// Scanners and mail gateways prepend junk. The spec says offset 0; reality
    /// does not always. A real PDF behind a preamble still has binary streams.
    #[test]
    fn identifies_a_pdf_behind_a_preamble() {
        let mut v = vec![b'\n'; 200];
        v.extend_from_slice(b"%PDF-1.4\nstream\n");
        v.extend_from_slice(&[0xFF, 0xFE, 0x00, 0x80]); // compressed stream bytes
        assert_eq!(identify(&v), Some(Container::Pdf));
    }

    /// The bound is the point: past the window this would be a search rather than
    /// a header check.
    #[test]
    fn a_pdf_marker_past_the_window_is_not_a_pdf() {
        let mut v = vec![0xFFu8; PDF_HEADER_WINDOW + 64];
        v.extend_from_slice(b"%PDF-1.4");
        assert_eq!(identify(&v), None);
    }

    /// The false positive that the UTF-8 condition exists to prevent. Refusing a
    /// readable document is worse than missing a binary one, because the missed
    /// case still has the base64 path and the `encoding=base64` header behind it.
    #[test]
    fn readable_prose_discussing_the_pdf_header_is_still_served() {
        let prose = b"The %PDF- header identifies the format; see section 4.";
        assert!(std::str::from_utf8(prose).is_ok(), "fixture must be valid UTF-8");
        assert_eq!(identify(prose), None);
    }

    /// But the offset-0 case is decisive on its own, UTF-8 or not — that is the
    /// door the confabulation case would otherwise walk through.
    #[test]
    fn a_valid_utf8_pdf_at_offset_zero_is_still_refused() {
        let uncompressed = b"%PDF-1.4\n1 0 obj\n<< /Type /Catalog >>\nendobj\n";
        assert!(
            std::str::from_utf8(uncompressed).is_ok(),
            "an uncompressed PDF can be entirely ASCII — that is the point"
        );
        assert_eq!(identify(uncompressed), Some(Container::Pdf));
    }

    #[test]
    fn tells_ooxml_from_a_plain_zip() {
        assert_eq!(
            identify(&zip_with_first_entry(b"[Content_Types].xml")),
            Some(Container::Ooxml)
        );
        assert_eq!(
            identify(&zip_with_first_entry(b"mimetype")),
            Some(Container::OpenDocument)
        );
        assert_eq!(identify(&zip_with_first_entry(b"readme.txt")), Some(Container::Zip));
    }

    /// A truncated header must degrade to `Zip`, never panic or read out of bounds.
    #[test]
    fn a_truncated_zip_header_degrades_instead_of_panicking() {
        assert_eq!(identify(b"PK\x03\x04short"), Some(Container::Zip));
        let mut lying = zip_with_first_entry(b"x");
        // Claim a 9999-byte name that is not there.
        lying[26] = 0x0F;
        lying[27] = 0x27;
        assert_eq!(identify(&lying), Some(Container::Zip));
    }

    #[test]
    fn identifies_the_long_unambiguous_signatures() {
        assert_eq!(identify(b"\x89PNG\r\n\x1A\n....."), Some(Container::Png));
        assert_eq!(identify(b"\xFF\xD8\xFF\xE0junk"), Some(Container::Jpeg));
        assert_eq!(identify(b"GIF89a..."), Some(Container::Gif));
        assert_eq!(identify(b"SQLite format 3\0rest"), Some(Container::Sqlite));
        assert_eq!(
            identify(b"\xD0\xCF\x11\xE0\xA1\xB1\x1A\xE1rest"),
            Some(Container::LegacyOffice)
        );
        assert_eq!(identify(b"\x1F\x8B\x08\x00"), Some(Container::Gzip));
        assert_eq!(identify(b"RIFF\x00\x00\x00\x00WEBPVP8 "), Some(Container::WebP));
    }

    /// The omission is deliberate and worth a test, so nobody "fixes" it later.
    #[test]
    fn two_letter_bitmap_magic_is_deliberately_not_matched() {
        assert_eq!(identify(b"BMW annual report 2026"), None);
    }

    #[test]
    fn plain_text_and_markup_are_not_containers() {
        assert_eq!(identify(b"# A markdown heading\n\nSome prose."), None);
        assert_eq!(identify(b"<?xml version=\"1.0\"?><root/>"), None);
        assert_eq!(identify(b"name,amount\nacme,100\n"), None);
        assert_eq!(identify(b"{\"key\": \"value\"}"), None);
    }

    #[test]
    fn too_short_to_judge_is_not_a_container() {
        assert_eq!(identify(b""), None);
        assert_eq!(identify(b"PK"), None);
    }

    /// Every variant must have a label and a class — a new container that forgets
    /// one is a refusal message with a hole in it.
    #[test]
    fn every_container_has_a_label_and_a_class() {
        let all = [
            Container::Pdf,
            Container::Ooxml,
            Container::OpenDocument,
            Container::LegacyOffice,
            Container::Zip,
            Container::Gzip,
            Container::Bzip2,
            Container::Xz,
            Container::SevenZip,
            Container::Rar,
            Container::Png,
            Container::Jpeg,
            Container::Gif,
            Container::Tiff,
            Container::WebP,
            Container::Sqlite,
        ];
        for c in all {
            assert!(!c.label().is_empty(), "{c:?} has no label");
            assert!(c.label().starts_with('a'), "{c:?} label must read as a noun phrase");
            let _ = c.class();
        }
    }

    // ---- classify_unrecognised ------------------------------------------
    //
    // Fixtures from the bug-hunt-2508 investigation (I-015), kept because they are
    // the cases the old guard called "an unrecognised binary format".

    /// Pass the real `valid_up_to`, the way `binary_guard` does.
    fn classify(bytes: &[u8]) -> Unrecognised {
        let off = std::str::from_utf8(bytes)
            .expect_err("fixture must not be valid UTF-8")
            .valid_up_to();
        classify_unrecognised(bytes, off)
    }

    fn text(bytes: &[u8]) -> TextEvidence {
        match classify(bytes) {
            Unrecognised::NonUtf8Text(e) => e,
            Unrecognised::Binary => panic!("expected text, got Binary"),
        }
    }

    /// `Tilbud til æble A/S` in Windows-1252 — the case in the field report.
    fn cp1252_danish() -> Vec<u8> {
        let mut v = b"Tilbud til ".to_vec();
        v.push(0xE6); // æ
        v.extend_from_slice(b"ble A/S\r\nPris: 100 kr\r\nSagsbeh: S");
        v.push(0xF8); // ø
        v.extend_from_slice(b"ren ");
        v.push(0xC5); // Å
        v.extend_from_slice(b"strup\r\n");
        v
    }

    fn utf16(s: &str, little_endian: bool, bom: bool) -> Vec<u8> {
        let mut v = Vec::new();
        if bom {
            v.extend_from_slice(if little_endian { &[0xFF, 0xFE] } else { &[0xFE, 0xFF] });
        }
        for u in s.encode_utf16() {
            v.extend_from_slice(&if little_endian {
                u.to_le_bytes()
            } else {
                u.to_be_bytes()
            });
        }
        v
    }

    #[test]
    fn danish_text_in_a_code_page_is_text_not_binary() {
        let ev = text(&cp1252_danish());
        assert_eq!(ev.looks_like, TextEncoding::SingleByte);
        assert_eq!(ev.bom, None);
        // 0xE6 is the æ, eleven bytes in.
        assert_eq!(ev.first_invalid_offset, 11);
        assert_eq!(ev.first_invalid_byte, 0xE6);
        assert!(ev.high_byte_ratio < 0.1, "prose is mostly ASCII: {}", ev.high_byte_ratio);
    }

    /// A spreadsheet export, which is the other half of the realistic set.
    #[test]
    fn a_code_page_csv_is_text() {
        let mut v = b"navn;beloeb\nF".to_vec();
        v.push(0xE5); // å
        v.extend_from_slice(b"rup;100\n");
        assert_eq!(text(&v).looks_like, TextEncoding::SingleByte);
    }

    /// One accented character in an otherwise ASCII file is still text. The old
    /// guard refused this too, and it is the likeliest shape of all.
    #[test]
    fn a_single_high_byte_is_still_text() {
        let ev = text(&[0x41, 0x42, 0xE5, 0x43, 0x44]);
        assert_eq!(ev.looks_like, TextEncoding::SingleByte);
        assert_eq!(ev.first_invalid_byte, 0xE5);
    }

    #[test]
    fn utf16_is_recognised_with_and_without_a_bom() {
        let s = "Tilbud til \u{e6}ble A/S\r\n";
        let le_bom = text(&utf16(s, true, true));
        assert_eq!(le_bom.looks_like, TextEncoding::Utf16Le);
        assert_eq!(le_bom.bom, Some(Bom::Utf16Le));

        let be_bom = text(&utf16(s, false, true));
        assert_eq!(be_bom.looks_like, TextEncoding::Utf16Be);
        assert_eq!(be_bom.bom, Some(Bom::Utf16Be));

        // BOM-less is the harder half: parity of the NUL bytes is the only signal.
        let le = text(&utf16(s, true, false));
        assert_eq!(le.looks_like, TextEncoding::Utf16Le);
        assert_eq!(le.bom, None);

        let be = text(&utf16(s, false, false));
        assert_eq!(be.looks_like, TextEncoding::Utf16Be);
        assert_eq!(be.bom, None);
    }

    /// The UTF-32 BOMs contain the UTF-16 ones. Getting the order wrong reports
    /// the wrong encoding to the administrator who has to fix the file.
    #[test]
    fn utf32_boms_are_not_mistaken_for_utf16() {
        let mut le = vec![0xFF, 0xFE, 0x00, 0x00];
        le.extend_from_slice(&[0x41, 0x00, 0x00, 0x00]);
        assert_eq!(text(&le).looks_like, TextEncoding::Utf32Le);

        let mut be = vec![0x00, 0x00, 0xFE, 0xFF];
        be.extend_from_slice(&[0x00, 0x00, 0x00, 0x41]);
        assert_eq!(text(&be).looks_like, TextEncoding::Utf32Be);
    }

    /// A file that claims UTF-8 and is not. The remedy differs from a file that
    /// never claimed anything, so the class is worth keeping distinct.
    #[test]
    fn a_utf8_bom_over_non_utf8_bytes_is_named_as_mislabelled() {
        let mut v = vec![0xEF, 0xBB, 0xBF];
        v.extend_from_slice(&cp1252_danish());
        let ev = text(&v);
        assert_eq!(ev.looks_like, TextEncoding::Utf8Mislabelled);
        assert_eq!(ev.bom, Some(Bom::Utf8));
    }

    #[test]
    fn genuinely_binary_bytes_are_still_binary() {
        // Stray control bytes are the signal prose never produces.
        let mut blob = vec![0x00, 0x01, 0x02, 0x03, 0x1B, 0x7F, 0xFF, 0xFE];
        blob.extend((0u8..=255).rev());
        assert_eq!(classify(&blob), Unrecognised::Binary);
    }

    /// A sparse region of NULs is not UTF-16 with an empty high byte. Without the
    /// "some non-NUL bytes, and those look like text" condition it would be.
    ///
    /// A file of *nothing but* NULs cannot be tested here, and that is worth
    /// knowing rather than working around: NUL is valid UTF-8, so such a file
    /// never reaches this function at all — it is served as text, by
    /// `render_envelope`, exactly as it was before this classifier existed.
    #[test]
    fn a_run_of_nuls_is_not_utf16() {
        assert!(std::str::from_utf8(&[0x00; 64]).is_ok(), "NUL is valid UTF-8");
        let mut mostly_nul = vec![0x00; 60];
        mostly_nul.extend_from_slice(&[0x01, 0x02, 0x1B, 0xFF]);
        assert_eq!(classify(&mostly_nul), Unrecognised::Binary);
    }

    /// Dense high bytes with no controls are not credible as prose, but they are
    /// not confidently anything either — say so rather than naming a code page.
    #[test]
    fn dense_high_bytes_are_text_of_an_unnamed_encoding() {
        let dense: Vec<u8> = (0..200).map(|i| 0x80 + (i % 0x40) as u8).collect();
        assert_eq!(text(&dense).looks_like, TextEncoding::Unknown);
    }
}
