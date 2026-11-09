use std::io::Write;

/// Pagina PDF interamente occupata da un JPEG.
pub struct JpegPage {
    pub data: Vec<u8>,
    pub width_pt: f64,
    pub height_pt: f64,
}

/// Byte di struttura PDF stimati per pagina.
pub const PAGE_OVERHEAD: usize = 400;

/// Dimensioni e componenti lette dall'intestazione, senza decodificare.
pub struct JpegInfo {
    pub width: u32,
    pub height: u32,
    pub components: u8,
}

pub fn jpeg_info(data: &[u8]) -> Option<JpegInfo> {
    let mut i = 2;
    while i + 9 < data.len() {
        if data[i] != 0xFF {
            return None;
        }
        let marker = data[i + 1];
        if marker == 0xFF {
            i += 1;
            continue;
        }
        let size = u16::from_be_bytes([data[i + 2], data[i + 3]]) as usize;
        let is_sof = (0xC0..=0xCF).contains(&marker) && !matches!(marker, 0xC4 | 0xC8 | 0xCC);
        if is_sof {
            return Some(JpegInfo {
                height: u16::from_be_bytes([data[i + 5], data[i + 6]]) as u32,
                width: u16::from_be_bytes([data[i + 7], data[i + 8]]) as u32,
                components: data[i + 9],
            });
        }
        i += 2 + size;
    }
    None
}

/// Assembla un PDF minimale con un JPEG a pagina, senza ricodificarli.
pub fn write_jpeg_pdf(pages: &[JpegPage]) -> Option<Vec<u8>> {
    let total: usize = pages.iter().map(|p| p.data.len() + PAGE_OVERHEAD).sum();
    let mut b: Vec<u8> = Vec::with_capacity(total + 1024);
    let mut offsets: Vec<usize> = Vec::with_capacity(2 + 3 * pages.len());

    b.extend_from_slice(b"%PDF-1.5\n%\xE2\xE3\xCF\xD3\n");
    write_obj(&mut b, &mut offsets, b"<</Type/Catalog/Pages 2 0 R>>");
    let kids: String = (0..pages.len()).map(|i| format!("{} 0 R ", 3 + 3 * i)).collect();
    let tree = format!("<</Type/Pages/Kids[{kids}]/Count {}>>", pages.len());
    write_obj(&mut b, &mut offsets, tree.as_bytes());

    for (i, page) in pages.iter().enumerate() {
        let info = jpeg_info(&page.data)?;
        let space = if info.components == 1 { "DeviceGray" } else { "DeviceRGB" };
        let first = 3 + 3 * i;
        let page_obj = format!(
            "<</Type/Page/Parent 2 0 R/MediaBox[0 0 {:.2} {:.2}]/Resources<</XObject<</I {} 0 R>>>>/Contents {} 0 R>>",
            page.width_pt, page.height_pt, first + 1, first + 2
        );
        write_obj(&mut b, &mut offsets, page_obj.as_bytes());

        let mut image = format!(
            "<</Type/XObject/Subtype/Image/Width {}/Height {}/ColorSpace/{space}/BitsPerComponent 8/Filter/DCTDecode/Length {}>>\nstream\n",
            info.width, info.height, page.data.len()
        )
        .into_bytes();
        image.extend_from_slice(&page.data);
        image.extend_from_slice(b"\nendstream");
        write_obj(&mut b, &mut offsets, &image);

        let content = format!("q {:.2} 0 0 {:.2} 0 0 cm /I Do Q", page.width_pt, page.height_pt);
        let contents = format!("<</Length {}>>\nstream\n{content}\nendstream", content.len());
        write_obj(&mut b, &mut offsets, contents.as_bytes());
    }

    let xref = b.len();
    let _ = write!(b, "xref\n0 {}\n0000000000 65535 f \n", offsets.len() + 1);
    for offset in &offsets {
        let _ = write!(b, "{offset:010} 00000 n \n");
    }
    let _ = write!(b, "trailer\n<</Size {}/Root 1 0 R>>\nstartxref\n{xref}\n%%EOF\n", offsets.len() + 1);
    Some(b)
}

fn write_obj(b: &mut Vec<u8>, offsets: &mut Vec<usize>, body: &[u8]) {
    offsets.push(b.len());
    let _ = write!(b, "{} 0 obj\n", offsets.len());
    b.extend_from_slice(body);
    b.extend_from_slice(b"\nendobj\n");
}
