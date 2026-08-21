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
}
