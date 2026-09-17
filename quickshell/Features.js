// Pure helpers for the panel's feature-spec list (M1: format and discovery).

function featureRows(response) {
    var features = response && Array.isArray(response.features) ? response.features : []
    return features.map(function(feature) {
        var reasons = Array.isArray(feature.reasons) ? feature.reasons : []
        var valid = feature.status === "valid"
        return {
            slug: feature.slug,
            title: feature.title,
            path: feature.path,
            status: feature.status,
            statusLabel: valid ? "valid" : "invalid: " + reasons.join("; "),
            valid: valid,
            reasons: reasons
        }
    })
}

function editorCommand(feature) {
    return ["omarchy-launch-editor", feature.path]
}
