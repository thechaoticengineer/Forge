// Pure model-catalogue presentation.
function catalogueProviderText(provider) {
    return provider.provider + ": " + provider.status + " · " + provider.model_count + " models"
        + (provider.cached_stale ? " · cached/stale" : "")
        + (provider.blocker ? " · " + provider.blocker.message
            : provider.error ? " · " + provider.error.message : "")
        + (provider.cache_error ? " · " + provider.cache_error : "")
}

function catalogueOptionText(option) {
    return option.provider + "/" + option.model + " · " + option.tier
        + " · " + option.availability + " · " + option.effort
        + " · configured revision " + option.policy_revision
        + (option.error ? " · " + option.error : "")
}

function catalogueStamp(unix) {
    return new Date(unix * 1000).toISOString().slice(0, 16).replace("T", " ") + "Z"
}

// Official metadata is descriptive only: freshness/provenance for routing
// context. Pricing appears only as a labelled API list rate, never as an
// inferred subscription charge or a configured preference.
function catalogueMetadataText(record) {
    const pricing = record.pricing
        ? record.pricing.label + " " + record.pricing.input + "/" + record.pricing.output
            + " " + record.pricing.currency + " " + record.pricing.unit
            + " · basis " + record.pricing.basis + " · as of " + record.pricing.as_of
        : "pricing unknown"
    return record.provider + "/" + record.model + " · " + record.provenance
        + " · verified " + catalogueStamp(record.verified_unix)
        + (record.removed ? " · removed (retained for audit)" : "")
        + (record.conflicts && record.conflicts.length ? " · conflict: discovered native support wins" : "")
        + " · " + pricing
}

function catalogueMetadataSourceText(source) {
    return source.url + (source.error
        ? " · error: " + source.error + " · failures " + source.failures
            + " · next attempt " + catalogueStamp(source.next_attempt_unix)
        : source.checked_unix ? " · ok · checked " + catalogueStamp(source.checked_unix) : "")
}

function catalogueMetadataSummaryText(meta) {
    if (!meta) return "Official metadata pending"
    return "Official metadata: " + meta.records + " records · " + meta.unknown_pricing + " unknown pricing · " + meta.negative + " negative-cached"
        + (meta.source_errors ? " · " + meta.source_errors + " source errors" : "")
        + (meta.refreshing ? " · refreshing…" : meta.last_refresh_unix
            ? " · checked " + catalogueStamp(meta.last_refresh_unix) + " (" + meta.last_requests + " requests)"
            : " · no research yet")
        + (meta.store_error ? " · " + meta.store_error : "")
}

// The message shown with ready AI tier suggestions: summary, sources, warnings,
// cost evidence and the reason for each model.
function suggestionMessage(resp) {
    return "AI tier suggestions — review and save to use them.\n" + resp.summary
        + (resp.sources && resp.sources.length ? "\n" + resp.sources.map(function(s) {
            return s.provider + ": " + s.status + " · " + s.url
        }).join("\n") : "")
        + (resp.warnings && resp.warnings.length ? "\n" + resp.warnings.join("\n") : "")
        + (resp.cost_evidence && resp.cost_evidence.length ? "\n" + resp.cost_evidence.map(function(c) {
            return c.provider + "/" + c.model + ": cost preference "
                + (c.relative_cost_preference === null ? "unknown" : c.relative_cost_preference)
                + (c.source_url ? " · standard API USD/1M tokens input " + c.input_per_million
                    + ", output " + c.output_per_million + " · " + c.source_url : "")
        }).join("\n") : "")
        + (resp.reasons && resp.reasons.length ? "\n" + resp.reasons.map(function(r) {
            return r.provider + "/" + r.model + ": " + r.rationale
        }).join("\n") : "")
}
