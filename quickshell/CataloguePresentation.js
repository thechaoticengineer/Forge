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
