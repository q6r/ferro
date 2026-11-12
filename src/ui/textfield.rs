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

    pub fn backspace(&mut self, word: bool) {
        if !self.has_selection() && self.cursor > 0 {
            self.anchor = if word { self.word_left() } else { self.cursor - 1 };
        }
        self.delete_selection();
    }

    pub fn delete(&mut self, word: bool) {
        if !self.has_selection() && self.cursor < self.len() {
            self.anchor = if word { self.word_right() } else { self.cursor + 1 };
        }
        self.delete_selection();
    }

    fn word_left(&self) -> usize {
        let chars: Vec<char> = self.text.chars().collect();
        let mut i = self.cursor;
        while i > 0 && !chars[i - 1].is_alphanumeric() {
            i -= 1;
        }
        while i > 0 && chars[i - 1].is_alphanumeric() {
            i -= 1;
        }
        i
    }

    fn word_right(&self) -> usize {
        let chars: Vec<char> = self.text.chars().collect();
        let mut i = self.cursor;
        while i < chars.len() && !chars[i].is_alphanumeric() {
            i += 1;
        }
        while i < chars.len() && chars[i].is_alphanumeric() {
            i += 1;
        }
        i
    }

    fn move_to(&mut self, pos: usize, extend: bool) {
        self.cursor = pos.min(self.len());
        if !extend {
            self.anchor = self.cursor;
        }
    }

    pub fn left(&mut self, extend: bool, word: bool) {
        if !extend && self.has_selection() {
            let (a, _) = self.selection();
            return self.move_to(a, false);
        }
        let pos = if word { self.word_left() } else { self.cursor.saturating_sub(1) };
        self.move_to(pos, extend);
    }

    pub fn right(&mut self, extend: bool, word: bool) {
        if !extend && self.has_selection() {
            let (_, b) = self.selection();
            return self.move_to(b, false);
        }
        let pos = if word { self.word_right() } else { self.cursor + 1 };
        self.move_to(pos, extend);
    }

    pub fn home(&mut self, extend: bool) {
        self.move_to(0, extend);
    }

    pub fn end(&mut self, extend: bool) {
        let len = self.len();
        self.move_to(len, extend);
    }

