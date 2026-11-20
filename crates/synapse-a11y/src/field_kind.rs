//! #2365: how a web form field takes a value, and which `<select>` option a
//! caller means. Pure value objects: no CDP, no OS. The CDP calls that use
//! them (`cdp_set_node_text`, `cdp_select_option`) stay in `cdp_action`.

#[cfg(test)]
#[path = "field_kind_table_2365.rs"]
mod field_kind_table_2365;

#[cfg(test)]
#[path = "dropdown_kind_table_2365b.rs"]
mod dropdown_kind_table_2365b;

/// Refusal for a hidden, disabled or read-only field (row F6).
const NOT_EDITABLE: &str = "the field is not editable: it is hidden, disabled or read-only";

/// Refusal when neither an option nor an index is given (row C11).
const OPTION_NEEDED: &str = "an option is needed: give option (its label or value) or option_index";

/// A "no such option" refusal lists at most this many labels (row C9).
const MAX_LISTED_LABELS: usize = 20;

/// How `set_field` must write one element.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SetMethod {
    /// Today's way: click, select all, insert the text.
    Typed,
    /// The element's native `value` setter, then `input` and `change`
    /// events, like a user's pick (date, time, colour, range...).
    Value,
    /// Not written by text; the reason says what to use instead.
    Refused(String),
}

/// One web element as `set_field` sees it: its tag, its `type` attribute,
/// whether it is content-editable, and whether it is locked (disabled or
/// read-only).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FieldKind {
    /// Lower case.
    tag: String,
    /// Lower case and trimmed; empty when the element has no `type`.
    input_type: String,
    content_editable: bool,
    /// `disabled` or `readOnly`.
    locked: bool,
}

impl FieldKind {
    /// Any element is a valid `FieldKind`; tag and type are compared without
    /// case.
    #[must_use]
    pub fn new(tag: &str, input_type: Option<&str>, content_editable: bool, locked: bool) -> Self {
        Self {
            tag: tag.trim().to_ascii_lowercase(),
            input_type: input_type.unwrap_or_default().trim().to_ascii_lowercase(),
            content_editable,
            locked,
        }
    }

    /// How `set_field` must write this element.
    #[must_use]
    pub fn set_method(&self) -> SetMethod {
        if self.locked {
            return SetMethod::Refused(NOT_EDITABLE.to_owned());
        }
        match self.tag.as_str() {
            "input" => self.input_method(),
            "textarea" => SetMethod::Typed,
            "select" => SetMethod::Refused(
                "a dropdown (<select>) is not set by text: use act select with option or option_index"
                    .to_owned(),
            ),
            _ if self.content_editable => SetMethod::Typed,
            _ => SetMethod::Refused(format!(
                "the <{}> element is not an editable field (not an input, textarea or content-editable element)",
                self.tag
            )),
        }
    }

    /// An `<input>` by its `type`. Text-like and unknown types are typed
    /// (browsers treat an unknown type as text).
    fn input_method(&self) -> SetMethod {
        match self.input_type.as_str() {
            "date" | "time" | "datetime-local" | "month" | "week" | "color" | "range" => {
                SetMethod::Value
            }
            "checkbox" | "radio" => SetMethod::Refused(
                "a checkbox or radio button is not set by text: use act check, uncheck or click"
                    .to_owned(),
            ),
            "file" => SetMethod::Refused(
                "a file field is not set by text: use the file upload tool (browser_file_upload)"
                    .to_owned(),
            ),
            "hidden" => SetMethod::Refused(NOT_EDITABLE.to_owned()),
            _ => SetMethod::Typed,
        }
    }
}

/// What a caller of `act select` asked for: an option's visible label or
/// value, or its zero-based index.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OptionChoice {
    /// Trimmed; `None` when absent or blank.
    option: Option<String>,
    option_index: Option<i32>,
}

impl OptionChoice {
    /// `option` = visible label or value; `option_index` = zero-based index
    /// (it wins when both are given).
    ///
    /// # Errors
    ///
    /// The refusal text when neither is given.
    pub fn new(option: Option<&str>, option_index: Option<i32>) -> Result<Self, String> {
        let option = option
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_owned);
        if option.is_none() && option_index.is_none() {
            return Err(OPTION_NEEDED.to_owned());
        }
        Ok(Self {
            option,
            option_index,
        })
    }

    /// The index in `options` (each `(label, value)`, in page order) the
    /// caller means: the index; else the one label equal to the text, then
    /// the one value equal to it (trimmed, without case).
    ///
    /// # Errors
    ///
    /// The refusal text: empty list, index out of range, two matches, or no
    /// match (listing at most 20 labels).
    pub fn pick(&self, options: &[(String, String)]) -> Result<usize, String> {
        if options.is_empty() {
            return Err("the dropdown has no options".to_owned());
        }
        if let Some(index) = self.option_index {
            return usize::try_from(index)
                .ok()
                .filter(|position| *position < options.len())
                .ok_or_else(|| {
                    format!(
                        "option_index {index} is out of range: the dropdown has {} options",
                        options.len()
                    )
                });
        }
        let Some(text) = self.option.as_deref() else {
            return Err(OPTION_NEEDED.to_owned());
        };
        let wanted = text.to_lowercase();
        let same = |part: &str| part.trim().to_lowercase() == wanted;
        let by_label: Vec<usize> = options
            .iter()
            .enumerate()
            .filter(|(_, (label, _))| same(label))
            .map(|(position, _)| position)
            .collect();
        let by_value: Vec<usize> = options
            .iter()
            .enumerate()
            .filter(|(_, (_, value))| same(value))
            .map(|(position, _)| position)
            .collect();
        match (by_label.as_slice(), by_value.as_slice()) {
            ([position], _) | ([], [position]) => Ok(*position),
            ([], []) => {
                let shown: Vec<&str> = options
                    .iter()
                    .take(MAX_LISTED_LABELS)
                    .map(|(label, _)| label.as_str())
                    .collect();
                let more = options.len().saturating_sub(MAX_LISTED_LABELS);
                let rest = if more > 0 {
                    format!(" and {more} more")
                } else {
                    String::new()
                };
                Err(format!(
                    "no option has the label or value \"{text}\"; the options are: {}{rest}",
                    shown.join(", ")
                ))
            }
            ([], many) => Err(format!(
                "{} options have the value \"{text}\": use option_index",
                many.len()
            )),
            (many, _) => Err(format!(
                "{} options have the label \"{text}\": use option_index",
                many.len()
            )),
        }
    }
}

/// How `act select` must pick on one web element (#2365b).
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Dropdown {
    /// A `<select>`: the browser's own dropdown (today's path).
    BrowserSelect,
    /// An element with `role="combobox"` that opens its own list of
    /// `role="option"` items: open it, click the option, read the text back.
    CustomComboBox,
    /// Not picked by `act select`; the reason says why or what to use.
    Refused(String),
}

/// One web element as `act select` sees it: its tag, its `role` attribute and
/// whether it is disabled (`disabled` or `aria-disabled="true"`).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DropdownKind {
    /// Lower case.
    tag: String,
    /// Lower case and trimmed; empty when the element has no `role`.
    role: String,
    disabled: bool,
}

impl DropdownKind {
    /// Any element is a valid `DropdownKind`; tag and role are compared
    /// without case.
    #[must_use]
    pub fn new(tag: &str, role: Option<&str>, disabled: bool) -> Self {
        Self {
            tag: tag.trim().to_ascii_lowercase(),
            role: role.unwrap_or_default().trim().to_ascii_lowercase(),
            disabled,
        }
    }

    /// Which way `act select` picks on this element.
    #[must_use]
    pub fn kind(&self) -> Dropdown {
        if self.tag == "select" {
            return Dropdown::BrowserSelect;
        }
        if self.role != "combobox" {
            return Dropdown::Refused(format!(
                "the <{}> element is not a dropdown (<select>)",
                self.tag
            ));
        }
        if self.tag == "input" {
            return Dropdown::Refused(
                "a type-to-filter combo box (<input role=\"combobox\">) is not picked by act select: type the text with set_field, then click the option"
                    .to_owned(),
            );
        }
        if self.disabled {
            return Dropdown::Refused("the combo box is disabled".to_owned());
        }
        Dropdown::CustomComboBox
    }
}

/// What a custom combo box shows after `act select` clicked one of its
/// options (#2365b): the chosen option's text and the trigger's own text,
/// both trimmed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ComboBoxReadback {
    chosen: String,
    shown: String,
}

impl ComboBoxReadback {
    #[must_use]
    pub fn new(chosen: &str, shown: &str) -> Self {
        Self {
            chosen: chosen.trim().to_owned(),
            shown: shown.trim().to_owned(),
        }
    }

    /// `Ok` when the trigger shows exactly the chosen option's text (letter
    /// case ignored).
    ///
    /// # Errors
    ///
    /// The text naming both: the chosen option and what the trigger shows.
    pub fn verdict(&self) -> Result<(), String> {
        if self.chosen.to_lowercase() == self.shown.to_lowercase() {
            return Ok(());
        }
        Err(format!(
            "the option \"{}\" was clicked but the combo box shows \"{}\"",
            self.chosen, self.shown
        ))
    }
}
