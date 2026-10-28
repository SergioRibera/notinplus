//! The editor's own text in Spanish.

use freya_markdown_editor::{Label, Labels};

thread_local! {
    static SPANISH: Labels = Labels::from_fn(spanish);
}

/// The Spanish labels, built once per thread.
pub(super) fn labels() -> Labels {
    SPANISH.with(Clone::clone)
}

#[allow(clippy::too_many_lines, reason = "one arm per label")]
const fn spanish(label: Label) -> &'static str {
    match label {
        Label::Undo => "Deshacer (Ctrl+Z)",
        Label::Redo => "Rehacer (Ctrl+Y)",
        Label::DefaultFont => "Fuente predeterminada",
        Label::FontSize => "Tamaño",
        Label::DefaultSize | Label::DefaultColor => "Predeterminado",
        Label::Bold => "Negrita (Ctrl+B)",
        Label::Italic => "Cursiva (Ctrl+I)",
        Label::Underline => "Subrayado (Ctrl+U)",
        Label::Strikethrough => "Tachado (Ctrl+Shift+X)",
        Label::InlineCode => "Código en línea (Ctrl+E)",
        Label::Highlight => "Resaltar (Ctrl+Shift+H)",
        Label::Superscript => "Superíndice (Ctrl+.)",
        Label::Subscript => "Subíndice (Ctrl+,)",
        Label::TextColor => "Color del texto",
        Label::BackgroundColor => "Color de fondo",
        Label::ClearFormatting => "Quitar formato (Ctrl+\\)",
        Label::Link => "Enlace (Ctrl+K)",
        Label::ApplyLink => "Aplicar",
        Label::RemoveLink | Label::RemoveProperty => "Quitar",
        Label::AlignLeft => "Alinear a la izquierda",
        Label::AlignCenter => "Centrar",
        Label::AlignRight => "Alinear a la derecha",
        Label::Justify => "Justificar",
        Label::Outdent => "Reducir sangría (Shift+Tab)",
        Label::Indent => "Aumentar sangría (Tab)",
        Label::Insert => "Insertar",
        Label::ReadingMode => "Pasar a modo lectura",
        Label::EditingMode => "Pasar a modo edición",
        Label::Text => "Texto",
        Label::Heading => "Título",
        Label::Heading1 => "Título 1",
        Label::Heading2 => "Título 2",
        Label::Heading3 => "Título 3",
        Label::Quote => "Cita",
        Label::Callout => "Aviso",
        Label::BulletedList => "Lista con viñetas",
        Label::NumberedList => "Lista numerada",
        Label::TodoList => "Lista de tareas",
        Label::Code => "Código",
        Label::Math | Label::Formula => "Fórmula",
        Label::Term => "Término",
        Label::Definition => "Definición",
        Label::Table => "Tabla",
        Label::Divider => "Separador",
        Label::CodeBlock => "Bloque de código",
        Label::MathBlock => "Bloque de fórmula",
        Label::TableOfContents => "Índice",
        Label::Note => "Nota",
        Label::Tip => "Consejo",
        Label::Important => "Importante",
        Label::Warning => "Advertencia",
        Label::Caution => "Precaución",
        Label::Blocks => "Bloques",
        Label::NoMatches => "Sin resultados",
        Label::PlainParagraph => "Párrafo simple",
        Label::BigHeading => "Título de sección grande",
        Label::MediumHeading => "Título de sección mediano",
        Label::SmallHeading => "Título de sección chico",
        Label::SimpleList => "Lista simple",
        Label::OrderedList => "Lista ordenada",
        Label::Checklist => "Casillas para tildar",
        Label::Quotation => "Texto citado",
        Label::TexFormula => "Fórmula TeX",
        Label::TermDetail => "Término de una lista de definiciones",
        Label::DefinitionDetail => "Definición de un término",
        Label::HorizontalRule => "Línea horizontal",
        Label::SmallTable => "Tabla de 3 × 3",
        Label::FromHeadings => "Armado con los títulos",
        Label::FrontMatter => "Metadatos",
        Label::EmptyContents => "Agregá títulos para armar el índice",
        Label::Block => "Bloque",
        Label::PlainText => "Texto plano",
        Label::SearchLanguage => "Buscá o escribí un lenguaje",
        Label::UseLanguage => "Usar",
        Label::PlainQuote => "Cita simple",
        Label::CopyCode => "Copiar",
        Label::Copied => "Copiado",
        Label::EditSource => "Editar",
        Label::DoneEditing => "Listo",
        Label::Rename => "Renombrar",
        Label::RenameHint => "Dejalo vacío para usar el nombre por defecto",
        Label::Comment => "Comentario",
        Label::CommentHint => "No se ve al leer",
        Label::Collapsible => "Sección plegable",
        Label::DetailsSummary => "Resumen",
        Label::Image => "Imagen",
        Label::ReplaceImage => "Reemplazar imagen",
        Label::AltText => "Descripción",
        Label::AltTextHint => "Describe la imagen para quien no puede verla",
        Label::RemoveImage => "Quitar imagen",
        Label::AddImage => "Agregá una imagen desde tus archivos",
        Label::Properties => "Propiedades",
        Label::AddProperty => "Agregar propiedad",
        Label::EditAsText => "Editar como texto",
        Label::ShowAsList => "Ver como lista",
        Label::PropertyName => "Nombre",
        Label::PropertyValue => "Valor",
        Label::Add => "Agregar",
        Label::Save => "Guardar",
        Label::Fraction => "Fracción",
        Label::SquareRoot => "Raíz cuadrada",
        Label::Power => "Potencia",
        Label::MathIndex => "Subíndice",
        Label::Sum => "Sumatoria",
        Label::Integral => "Integral definida",
        Label::Matrix => "Matriz",
        Label::Diagram => "Diagrama",
        Label::DiagramDetail => "Diagrama de flujo que se edita como texto",
        Label::DiagramTemplate => {
            "flowchart TD\n    inicio[Inicio] --> pregunta{¿Decisión?}\n    pregunta -->|Sí| hacer[Hacerlo]\n    pregunta -->|No| fin[Terminar]"
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_label_is_translated() {
        let labels = labels();
        let untranslated: Vec<_> = Label::ALL
            .into_iter()
            .filter(|label| labels.get(*label) == label.english())
            .collect();
        assert!(untranslated.is_empty(), "{untranslated:?}");
    }
}
