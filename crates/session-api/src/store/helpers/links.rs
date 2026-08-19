pub(super) fn extend_unique(
    target: &mut Vec<String>,
    incoming: Vec<String>,
) {
    for value in incoming {
        if !target.iter().any(|existing| existing == &value) {
            target.push(value);
        }
    }
}
