pub const SUPPORTED_EXTENSIONS: &[&str] = &[
    "pdf", "doc", "dot", "docx", "docm", "dotx", "dotm", "xls", "xlt", "xlsx", "xlsm", "xlsb",
    "xltx", "xltm", "xlam", "ppt", "pps", "pptx", "pptm", "ppsx", "potx", "potm", "odt", "ods",
    "odp", "hwp", "hwpx", "pages", "numbers", "key", "fodt", "fods", "fodp", "epub", "zip", "tar",
    "gz", "tgz", "7z", "rtf", "txt", "text", "md", "markdown", "mdx", "rst", "org", "adoc", "tex",
    "latex", "typ", "typst", "json", "yaml", "yml", "toml", "csv", "tsv", "log", "jsonl", "ndjson",
    "html", "htm", "xhtml", "xml", "svg", "opml", "fb2", "docbook", "dbk", "jats", "nxml", "tei",
    "dita", "ditamap", "eml", "emlx", "mbox", "msg", "ipynb", "ris", "enw", "nbib", "bib",
    "bibtex", "csl", "ics", "ical", "ifb", "vcf", "vcard", "png", "jpg", "jpeg", "webp", "gif",
    "bmp", "tif", "tiff",
];

const DOCX_FAMILY: &[&str] = &["docx", "docm", "dotx", "dotm"];
const LEGACY_DOC_FAMILY: &[&str] = &["doc", "dot"];
const XLSX_FAMILY: &[&str] = &["xlsx", "xlsm", "xlsb", "xltx", "xltm", "xlam"];
const LEGACY_XLS_FAMILY: &[&str] = &["xls", "xlt"];
const PPTX_FAMILY: &[&str] = &["pptx", "pptm", "ppsx", "potx", "potm"];
const LEGACY_PPT_FAMILY: &[&str] = &["ppt", "pps"];
const ODF_FAMILY: &[&str] = &["odt", "ods", "odp"];
const FLAT_ODF_FAMILY: &[&str] = &["fodt", "fods", "fodp"];
const HTML_FAMILY: &[&str] = &["html", "htm", "xhtml"];
const PLAIN_TEXT_FAMILY: &[&str] = &[
    "txt", "text", "md", "markdown", "mdx", "rst", "org", "adoc", "tex", "latex", "typ", "typst",
    "json", "yaml", "yml", "toml", "csv", "tsv", "log", "jsonl", "ndjson",
];
const XML_FAMILY: &[&str] = &[
    "xml", "svg", "opml", "fb2", "docbook", "dbk", "jats", "nxml", "tei", "dita", "ditamap",
];
const IMAGE_FAMILY: &[&str] = &["png", "jpg", "jpeg", "webp", "gif", "bmp", "tif", "tiff"];
const ICS_FAMILY: &[&str] = &["ics", "ical", "ifb"];
const VCARD_FAMILY: &[&str] = &["vcf", "vcard"];

pub fn is_docx_family(ext: &str) -> bool {
    DOCX_FAMILY.contains(&ext)
}

pub fn is_legacy_doc_family(ext: &str) -> bool {
    LEGACY_DOC_FAMILY.contains(&ext)
}

pub fn is_xlsx_family(ext: &str) -> bool {
    XLSX_FAMILY.contains(&ext)
}

pub fn is_legacy_xls_family(ext: &str) -> bool {
    LEGACY_XLS_FAMILY.contains(&ext)
}

pub fn is_pptx_family(ext: &str) -> bool {
    PPTX_FAMILY.contains(&ext)
}

pub fn is_legacy_ppt_family(ext: &str) -> bool {
    LEGACY_PPT_FAMILY.contains(&ext)
}

pub fn is_odf_family(ext: &str) -> bool {
    ODF_FAMILY.contains(&ext)
}

pub fn is_flat_odf_family(ext: &str) -> bool {
    FLAT_ODF_FAMILY.contains(&ext)
}

pub fn is_html_family(ext: &str) -> bool {
    HTML_FAMILY.contains(&ext)
}

pub fn is_plain_text_family(ext: &str) -> bool {
    PLAIN_TEXT_FAMILY.contains(&ext)
}

pub fn is_xml_family(ext: &str) -> bool {
    XML_FAMILY.contains(&ext)
}

pub fn is_image_family(ext: &str) -> bool {
    IMAGE_FAMILY.contains(&ext)
}

pub fn is_ics_family(ext: &str) -> bool {
    ICS_FAMILY.contains(&ext)
}

pub fn is_vcard_family(ext: &str) -> bool {
    VCARD_FAMILY.contains(&ext)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supported_extensions_include_extended_text_and_archive_formats() {
        for ext in [
            "mdx", "rst", "org", "tex", "latex", "typ", "typst", "7z", "hwp",
        ] {
            assert!(SUPPORTED_EXTENSIONS.contains(&ext));
        }
    }

    #[test]
    fn plain_text_family_includes_extended_markup_formats() {
        for ext in ["mdx", "rst", "org", "tex", "latex", "typ", "typst"] {
            assert!(is_plain_text_family(ext));
        }
    }
}
