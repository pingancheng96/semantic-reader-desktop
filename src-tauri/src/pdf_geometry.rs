use crate::sqlite::Result;
use serde_json::Value;
use std::path::Path;

#[cfg(target_os = "macos")]
#[link(name = "PDFKit", kind = "framework")]
extern "C" {}

#[cfg(target_os = "macos")]
pub fn word_boxes(path: &Path, page_number: usize) -> Result<Value> {
    use objc2::{
        class, msg_send,
        rc::{autoreleasepool, Allocated, Retained},
        runtime::AnyObject,
    };
    use objc2_foundation::{NSRange, NSRect, NSString, NSURL};
    use serde_json::json;
    use unicode_segmentation::UnicodeSegmentation;

    autoreleasepool(|_| {
        let path = path.to_str().ok_or("PDF 路径无效")?;
        let url = NSURL::fileURLWithPath(&NSString::from_str(path));
        // PDFKit objects are local to this worker and only used with their documented selectors.
        unsafe {
            let allocated: Allocated<AnyObject> = msg_send![class!(PDFDocument), alloc];
            let document: Option<Retained<AnyObject>> = msg_send![allocated, initWithURL: &*url];
            let document = document.ok_or("PDFKit 无法打开原始 PDF")?;
            let page: Option<Retained<AnyObject>> =
                msg_send![&*document, pageAtIndex: page_number - 1];
            let page = page.ok_or("PDF 页码无效")?;
            let text: Option<Retained<NSString>> = msg_send![&*page, string];
            let Some(text) = text else {
                return Ok(json!([]));
            };
            let text = text.to_string();
            let mut result = Vec::new();
            let mut previous_byte = 0;
            let mut utf16 = 0;
            for (byte, word) in text.unicode_word_indices() {
                utf16 += text[previous_byte..byte].encode_utf16().count();
                let length = word.encode_utf16().count();
                let range = NSRange::new(utf16, length);
                previous_byte = byte;
                autoreleasepool(|_| {
                    let selection: Option<Retained<AnyObject>> =
                        msg_send![&*page, selectionForRange: range];
                    if let Some(selection) = selection {
                        let bounds: NSRect = msg_send![&*selection, boundsForPage: &*page];
                        let x = bounds.origin.x;
                        let y = bounds.origin.y;
                        let w = bounds.size.width;
                        let h = bounds.size.height;
                        if [x, y, w, h].iter().all(|n| n.is_finite()) && w > 0.0 && h > 0.0 {
                            result.push(json!({"word":word,"rect":[x,y,x+w,y+h]}));
                            return;
                        }
                    }
                    result.push(json!({"word":word,"rect":null}));
                });
            }
            Ok(json!(result))
        }
    })
}

#[cfg(not(target_os = "macos"))]
pub fn word_boxes(_path: &Path, _page_number: usize) -> Result<Value> {
    Ok(serde_json::json!([]))
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use std::fmt::Write;

    #[test]
    fn native_pdf_boxes_use_pdf_word_positions() {
        let path =
            std::env::temp_dir().join(format!("semantic-geometry-{}.pdf", uuid::Uuid::new_v4()));
        let stream = "BT /F1 20 Tf 20 150 Td (An attempt to read.) Tj ET";
        let contents = format!(
            "<< /Length {} >>\nstream\n{stream}\nendstream",
            stream.len()
        );
        let objects = [
            "<< /Type /Catalog /Pages 2 0 R >>",
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 220] /Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R >>",
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
            &contents,
        ];
        let mut pdf = String::from("%PDF-1.4\n");
        let mut offsets = Vec::new();
        for (index, object) in objects.iter().enumerate() {
            offsets.push(pdf.len());
            writeln!(pdf, "{} 0 obj\n{object}\nendobj", index + 1).unwrap();
        }
        let xref = pdf.len();
        pdf.push_str("xref\n0 6\n0000000000 65535 f \n");
        for offset in offsets {
            writeln!(pdf, "{offset:010} 00000 n ").unwrap();
        }
        writeln!(
            pdf,
            "trailer\n<< /Size 6 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF"
        )
        .unwrap();
        std::fs::write(&path, pdf).unwrap();
        let boxes = word_boxes(&path, 1);
        std::fs::remove_file(path).unwrap();
        let boxes = boxes.unwrap();
        assert_eq!(boxes[1]["word"], "attempt");
        assert!(boxes[1]["rect"][0].as_f64().unwrap() > 20.0);
        assert!(boxes[1]["rect"][2].as_f64().unwrap() < boxes[2]["rect"][0].as_f64().unwrap());
    }
}
