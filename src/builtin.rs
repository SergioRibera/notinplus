//! Faces of builtin kinds that need more than the generic canvas: the
//! PDF face lays the document under the ink.

pub mod pdf_face;

pub use pdf_face::PdfFace;
