//! Borrowed iteration over an already checked start tag. No XML admission occurs here.
use super::{Attribute, Span, Xml, whitespace};
use std::ops::Range;
pub struct Attributes<'a> {
    xml: &'a Xml,
    front: usize,
    back: usize,
    remaining: usize,
    normalized: Range<usize>,
}
impl<'a> Attributes<'a> {
    pub(super) fn new(xml: &'a Xml, span: Span, len: usize, normalized: Range<usize>) -> Self {
        Self { xml, front: span.start as usize, back: span.end as usize, remaining: len, normalized }
    }
    pub(super) fn next_spans(&mut self) -> Option<(Span, Span, u8)> {
        if self.remaining == 0 { return None; }
        let b = self.xml.text.as_bytes();
        while self.front < self.back && whitespace(b[self.front]) { self.front += 1; }
        let start = self.front;
        while self.front < self.back && !whitespace(b[self.front]) && b[self.front] != b'=' { self.front += 1; }
        let key = Span { start: start as u32, end: self.front as u32 };
        while whitespace(b[self.front]) { self.front += 1; }
        self.front += 1; // '=' was checked during admission.
        while whitespace(b[self.front]) { self.front += 1; }
        let quote = b[self.front]; self.front += 1;
        let value_start = self.front;
        self.front += memchr::memchr(quote, &b[self.front..self.back])?;
        let value = Span { start: value_start as u32, end: self.front as u32 };
        self.front += 1; self.remaining -= 1;
        Some((key, value, quote))
    }
    fn view(&self, key: Span, value: Span, quote: u8) -> Attribute<'a> {
        Attribute { name: &self.xml.text[key.range()], span: value, quote,
            value: self.xml.attribute_value(value, self.normalized.clone()) }
    }
}
impl<'a> Iterator for Attributes<'a> {
    type Item = Attribute<'a>;
    fn next(&mut self) -> Option<Self::Item> {
        let (key, value, quote) = self.next_spans()?;
        Some(self.view(key, value, quote))
    }
    fn size_hint(&self) -> (usize, Option<usize>) { (self.remaining, Some(self.remaining)) }
}
impl ExactSizeIterator for Attributes<'_> {}
impl DoubleEndedIterator for Attributes<'_> {
    fn next_back(&mut self) -> Option<Self::Item> {
        if self.remaining == 0 { return None; }
        let b = self.xml.text.as_bytes();
        while self.back > self.front && whitespace(b[self.back - 1]) { self.back -= 1; }
        self.back -= 1; let quote = b[self.back]; let end = self.back;
        self.back = self.front + memchr::memrchr(quote, &b[self.front..self.back])?;
        let value = Span { start: (self.back + 1) as u32, end: end as u32 };
        while whitespace(b[self.back - 1]) { self.back -= 1; }
        self.back -= 1; // '='
        while whitespace(b[self.back - 1]) { self.back -= 1; }
        let end = self.back;
        while self.back > self.front && !whitespace(b[self.back - 1]) { self.back -= 1; }
        self.remaining -= 1;
        Some(self.view(Span { start: self.back as u32, end: end as u32 }, value, quote))
    }
}
impl std::iter::FusedIterator for Attributes<'_> {}
