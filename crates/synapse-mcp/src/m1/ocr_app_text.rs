//! #314: the app's own text wins over an OCR misread. A `read_text` with a
//! window target asks the window's accessibility tree (a web element: the
//! page) which text sits where the OCR found words. When one text box holds
//! OCR words, is not much taller than them and its text is of a similar
//! length, the reply carries the app's text for those words, marked
//! `confidence_source: uia`. Pure, except `for_window_read` (OS reads).

use synapse_core::{AccessibleNode, ConfidenceSource, OcrResult, OcrWord, Point, Rect};

#[cfg(test)]
#[path = "ocr_app_text_table_314.rs"]
mod ocr_app_text_table_314;

/// A text box explains OCR words only when it is at most this many times as
/// tall as the tallest of them: a window, pane or editor is refused, a label,
/// button or text run is kept.
const LINE_HEIGHT_FACTOR: i32 = 3;

/// A text replaces OCR words only when neither is more than this many times
/// as long as the other (spaces and direction marks not counted): the label
/// "Bold" does not replace the icon letter "B".
const MAX_LENGTH_FACTOR: usize = 2;

/// Invisible direction marks an app may put around a text (FM-10: U+2066
/// before and U+2069 after "-₪528.53"). They are not shown, so they are not
/// part of the text read.
const DIRECTION_MARKS: [char; 12] = [
    '\u{200E}', '\u{200F}', '\u{202A}', '\u{202B}', '\u{202C}', '\u{202D}', '\u{202E}', '\u{2066}',
    '\u{2067}', '\u{2068}', '\u{2069}', '\u{061C}',
];

/// The texts an app shows, each with its box in the OCR reply's coordinates.
/// The default (no text) changes no reply.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AppTextCheck {
    texts: Vec<AppText>,
}

/// One shown text: cleaned (no direction marks, one space between words),
/// never empty, box with an area.
#[derive(Clone, Debug, Eq, PartialEq)]
struct AppText {
    text: String,
    bbox: Rect,
}

impl AppTextCheck {
    /// JavaScript run on a web element to read its page text: the rendered
    /// text (a DOM text node: its text), and nothing for a form field (a
    /// password must never come back).
    pub const PAGE_TEXT_FUNCTION: &'static str = "el => (el.matches && el.matches('input, textarea, select')) ? '' : String(el.innerText ?? el.textContent ?? '')";

    /// From texts whose boxes are already in the OCR reply's coordinates (a
    /// web element's page text; its box is the element bitmap). A text that is
    /// empty after cleaning, or a box with no area, is left out.
    #[must_use]
    pub fn new(texts: Vec<(String, Rect)>) -> Self {
        Self {
            texts: texts
                .into_iter()
                .filter_map(|(text, bbox)| AppText::new(&text, bbox))
                .collect(),
        }
    }

    /// From a window's accessibility nodes (screen boxes). `word_origin` is
    /// the screen point of the OCR reply's (0, 0). A node shows its `value`
    /// when that is not empty (an edit box shows its content; its `name` is
    /// its label), otherwise its `name`.
    #[must_use]
    pub fn from_nodes(nodes: &[AccessibleNode], word_origin: Point) -> Self {
        Self {
            texts: nodes
                .iter()
                .filter_map(|node| {
                    let text = node
                        .value
                        .as_deref()
                        .filter(|value| !AppText::clean(value).is_empty())
                        .unwrap_or(&node.name);
                    let bbox = Rect {
                        x: node.bbox.x.saturating_sub(word_origin.x),
                        y: node.bbox.y.saturating_sub(word_origin.y),
                        w: node.bbox.w,
                        h: node.bbox.h,
                    };
                    AppText::new(text, bbox)
                })
                .collect(),
        }
    }

    /// Reads the accessibility tree of the window a `read_text` captured.
    /// No text (the OCR reply is kept as it is) for a screen read, a
    /// synthetic read, or when the window bounds or its tree cannot be read.
    #[cfg(windows)]
    #[must_use]
    pub fn for_window_read(request: &super::ResolvedReadTextRequest) -> Self {
        Self::window_texts(request).unwrap_or_default()
    }

    /// Not Windows: there is no accessibility tree to read.
    #[cfg(not(windows))]
    #[must_use]
    pub fn for_window_read(_request: &super::ResolvedReadTextRequest) -> Self {
        Self::default()
    }

    /// The window's texts with their boxes moved into the reply's coordinates:
    /// the captured pixels start at the window frame + `window_region`, the
    /// reply's words at `request.region`.
    #[cfg(windows)]
    fn window_texts(request: &super::ResolvedReadTextRequest) -> Option<Self> {
        if request.synthetic {
            return None;
        }
        let (hwnd, window_region) = match request.capture_source {
            super::ReadTextCaptureSource::Screen => return None,
            super::ReadTextCaptureSource::Window {
                hwnd,
                window_region,
            } => (hwnd, window_region),
            super::ReadTextCaptureSource::WholeWindow { hwnd } => (
                hwnd,
                Rect {
                    x: 0,
                    y: 0,
                    w: 0,
                    h: 0,
                },
            ),
        };
        let frame = synapse_capture::window_capture_bounds(hwnd).ok()?;
        let tree =
            synapse_a11y::snapshot_window_from_hwnd(hwnd, super::MAX_PUBLIC_A11Y_SNAPSHOT_DEPTH)
                .ok()?;
        let word_origin = Point {
            x: frame
                .x
                .saturating_add(window_region.x)
                .saturating_sub(request.region.x),
            y: frame
                .y
                .saturating_add(window_region.y)
                .saturating_sub(request.region.y),
        };
        Some(Self::from_nodes(&tree.nodes, word_origin))
    }

    /// The OCR reply with the app's text in place of the OCR words it
    /// explains; the reply unchanged when no text explains any word.
    #[must_use]
    pub fn apply(&self, result: OcrResult) -> OcrResult {
        let owners: Vec<Option<usize>> = result
            .words
            .iter()
            .map(|word| self.owner_of(word.bbox))
            .collect();
        let replacements: Vec<Option<OcrWord>> = (0..self.texts.len())
            .map(|index| self.replacement(index, &result.words, &owners))
            .collect();
        if replacements.iter().all(Option::is_none) {
            return result;
        }
        let mut placed = vec![false; replacements.len()];
        let mut words = Vec::with_capacity(result.words.len());
        for (word, owner) in result.words.iter().zip(&owners) {
            let replacement = owner.and_then(|index| {
                replacements
                    .get(index)?
                    .as_ref()
                    .map(|app_word| (index, app_word))
            });
            match replacement {
                None => words.push(word.clone()),
                Some((index, app_word)) => {
                    // The merged word takes the place of its group's first word.
                    if let Some(done) = placed.get_mut(index)
                        && !*done
                    {
                        *done = true;
                        words.push(app_word.clone());
                    }
                }
            }
        }
        Self::with_words(result, words)
    }

    /// The text whose box holds the word's centre: the smallest such box, the
    /// first of equal ones.
    fn owner_of(&self, word: Rect) -> Option<usize> {
        let centre = Point {
            x: word.x.saturating_add(word.w / 2),
            y: word.y.saturating_add(word.h / 2),
        };
        let mut owner: Option<(usize, i64)> = None;
        for (index, app) in self.texts.iter().enumerate() {
            if app.contains(centre) && owner.is_none_or(|(_, area)| app.area() < area) {
                owner = Some((index, app.area()));
            }
        }
        owner.map(|(index, _)| index)
    }

    /// The app word for text `index` when it explains its OCR words: its box
    /// is at most `LINE_HEIGHT_FACTOR` times as tall as the tallest of them,
    /// and the lengths are within `MAX_LENGTH_FACTOR`.
    fn replacement(
        &self,
        index: usize,
        words: &[OcrWord],
        owners: &[Option<usize>],
    ) -> Option<OcrWord> {
        let app = self.texts.get(index)?;
        let group: Vec<&OcrWord> = words
            .iter()
            .zip(owners)
            .filter(|(_, owner)| **owner == Some(index))
            .map(|(word, _)| word)
            .collect();
        let tallest = group.iter().map(|word| word.bbox.h).max()?;
        let ocr_text = group
            .iter()
            .map(|word| word.text.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        if app.bbox.h > tallest.saturating_mul(LINE_HEIGHT_FACTOR) || !app.similar_length(&ocr_text)
        {
            return None;
        }
        let bbox = group.iter().map(|word| word.bbox).reduce(Self::union)?;
        Some(OcrWord {
            text: app.text.clone(),
            bbox,
            confidence: 1.0,
            confidence_source: ConfidenceSource::Uia,
        })
    }

    /// The reply with new words: `full_text` joined again, confidence the mean
    /// of the words, source the least trusted of the words (#311 rule).
    fn with_words(result: OcrResult, words: Vec<OcrWord>) -> OcrResult {
        let full_text = words
            .iter()
            .map(|word| word.text.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        let count = u16::try_from(words.len()).unwrap_or(u16::MAX).max(1);
        let confidence = words.iter().map(|word| word.confidence).sum::<f32>() / f32::from(count);
        let confidence_source = words
            .iter()
            .map(|word| word.confidence_source)
            .reduce(ConfidenceSource::least_trusted)
            .unwrap_or_default();
        OcrResult {
            full_text,
            words,
            confidence,
            confidence_source,
            ..result
        }
    }

    /// The smallest box around both.
    fn union(a: Rect, b: Rect) -> Rect {
        let x = a.x.min(b.x);
        let y = a.y.min(b.y);
        let right = a.x.saturating_add(a.w).max(b.x.saturating_add(b.w));
        let bottom = a.y.saturating_add(a.h).max(b.y.saturating_add(b.h));
        Rect {
            x,
            y,
            w: right.saturating_sub(x),
            h: bottom.saturating_sub(y),
        }
    }
}

impl AppText {
    /// `None` when the text is empty after cleaning or the box has no area.
    fn new(text: &str, bbox: Rect) -> Option<Self> {
        let text = Self::clean(text);
        (!text.is_empty() && bbox.w > 0 && bbox.h > 0).then_some(Self { text, bbox })
    }

    /// Without direction marks, one space between words, no outer spaces.
    fn clean(text: &str) -> String {
        text.chars()
            .filter(|ch| !DIRECTION_MARKS.contains(ch))
            .collect::<String>()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Left and top edges inside, right and bottom edges outside.
    const fn contains(&self, point: Point) -> bool {
        self.bbox.x <= point.x
            && point.x < self.bbox.x.saturating_add(self.bbox.w)
            && self.bbox.y <= point.y
            && point.y < self.bbox.y.saturating_add(self.bbox.h)
    }

    fn area(&self) -> i64 {
        i64::from(self.bbox.w) * i64::from(self.bbox.h)
    }

    /// Neither text more than `MAX_LENGTH_FACTOR` times as long as the other.
    fn similar_length(&self, ocr_text: &str) -> bool {
        let app = Self::letter_count(&self.text);
        let ocr = Self::letter_count(ocr_text);
        app <= ocr.saturating_mul(MAX_LENGTH_FACTOR) && ocr <= app.saturating_mul(MAX_LENGTH_FACTOR)
    }

    /// Letters, digits and signs: no spaces, no direction marks.
    fn letter_count(text: &str) -> usize {
        text.chars()
            .filter(|ch| !ch.is_whitespace() && !DIRECTION_MARKS.contains(ch))
            .count()
    }
}
