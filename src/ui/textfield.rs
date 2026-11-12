//! Campo de texto de uma linha: cursor, seleção e edição. Os índices são
//! em caracteres (não bytes), para lidar bem com acentos.

#[derive(Default, Clone)]
pub struct TextField {
    pub text: String,
    /// Posição do cursor.
    pub cursor: usize,
    /// Outra ponta da seleção (igual ao cursor quando não há seleção).
    pub anchor: usize,
}

impl TextField {
    fn len(&self) -> usize {
        self.text.chars().count()
    }

    fn byte(&self, idx: usize) -> usize {
        self.text.char_indices().nth(idx).map_or(self.text.len(), |(b, _)| b)
    }

    pub fn set(&mut self, text: &str) {
        self.text = text.to_string();
        self.select_all();
    }

    pub fn select_all(&mut self) {
        self.anchor = 0;
        self.cursor = self.len();
    }

    pub fn selection(&self) -> (usize, usize) {
        (self.cursor.min(self.anchor), self.cursor.max(self.anchor))
    }

    pub fn has_selection(&self) -> bool {
        self.cursor != self.anchor
    }

    pub fn selected_text(&self) -> &str {
        let (a, b) = self.selection();
        &self.text[self.byte(a)..self.byte(b)]
    }

    fn delete_selection(&mut self) {
        let (a, b) = self.selection();
        let (ba, bb) = (self.byte(a), self.byte(b));
        self.text.replace_range(ba..bb, "");
        self.cursor = a;
        self.anchor = a;
    }

    pub fn insert(&mut self, s: &str) {
        let s: String = s.chars().filter(|c| !c.is_control()).collect();
        self.delete_selection();
        let b = self.byte(self.cursor);
        self.text.insert_str(b, &s);
        self.cursor += s.chars().count();
        self.anchor = self.cursor;
    }

