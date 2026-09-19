//! Compact projections of architecture records for provider prompts.
//!
//! Every function clones its input: storage, publication and validity checks
//! keep the complete records, and only the text a provider reads is reduced.

use serde_json::Value;

/// Guidance and agreement records carry a `relevant_inputs` fingerprint that
/// copies the stage text a prompt already contains and grows with every
/// dependency. No provider prompt needs it.
const FINGERPRINT: &str = "relevant_inputs";

fn strip_record(record: &mut Value) {
    if let Some(record) = record.as_object_mut() { record.remove(FINGERPRINT); }
}

/// One guidance or agreement record for a prompt.
pub(crate) fn agreement(record: &Value) -> Value {
    let mut view = record.clone();
    strip_record(&mut view);
    view
}

/// Guidance for a prompt: accepts a map of guidance records keyed by stage or
/// one guidance record, and returns it without the input fingerprint.
pub(crate) fn guidance(map_or_record: &Value) -> Value {
    if map_or_record.get(FINGERPRINT).is_some() { return agreement(map_or_record); }
    let mut view = map_or_record.clone();
    if let Some(records) = view.as_object_mut() { records.values_mut().for_each(strip_record); }
    view
}

/// A checkpoint for a prompt: every record in `guidance` and `agreements`
/// loses its input fingerprint, everything else is kept as is.
pub(crate) fn checkpoint(cp: &Value) -> Value {
    let mut view = cp.clone();
    for group in ["guidance", "agreements"] {
        if let Some(records) = view.get_mut(group).and_then(Value::as_object_mut) {
            records.values_mut().for_each(strip_record);
        }
    }
    view
}

#[cfg(test)]
#[path = "prompt_view_tests.rs"]
mod tests;
