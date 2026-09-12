//! Reproducible draft preferences from published rates, never an AI price guess.
use crate::catalogue::{Entry, Provider, identifier};
use crate::metadata::{CurlFetch, Document, Fetch, retrieve};
use serde_json::{Value, json};
use std::collections::BTreeMap;

const PRICE_URL: &str = "https://developers.openai.com/api/docs/pricing.md";

fn dollars(cell: &str) -> Option<f64> {
    let value = cell.strip_prefix('$')?;
    if value.is_empty() || !value.bytes().all(|c| c.is_ascii_digit() || c == b'.') { return None; }
    value.parse::<f64>().ok().filter(|n| n.is_finite() && *n >= 0.0)
}

fn rates(body: &[u8]) -> Result<BTreeMap<String, (f64, f64)>, String> {
    let text = std::str::from_utf8(body).map_err(|_| "invalid pricing document")?;
    if !text.contains("Prices per 1M tokens.") { return Err("unknown pricing units".into()); }
    let section = text.split_once("### Standard pricing data\n").ok_or("standard pricing table missing")?.1;
    let section = section.split("\n### ").next().unwrap_or(section);
    let mut rows = section.lines().map(str::trim).skip_while(|line| !line.starts_with("| Model |"));
    let header: Vec<_> = rows.next().ok_or("pricing header missing")?.trim_matches('|').split('|').map(str::trim).collect();
    let input = header.iter().position(|c| *c == "Short context input").ok_or("input column missing")?;
    let output = header.iter().position(|c| *c == "Short context output").ok_or("output column missing")?;
    let mut rates = BTreeMap::new();
    for line in rows.take_while(|line| line.starts_with('|')) {
        let cells: Vec<_> = line.trim_matches('|').split('|').map(str::trim).collect();
        if cells.len() != header.len() { continue; }
        let id = cells[0].strip_suffix(" (<272K context length)").unwrap_or(cells[0]);
        if let (true, Some(i), Some(o)) = (identifier(id), dollars(cells[input]), dollars(cells[output])) {
            if rates.insert(id.into(), (i, o)).is_some() { return Err("duplicate standard model price".into()); }
        }
    }
    if rates.is_empty() { return Err("no supported standard prices found".into()); }
    Ok(rates)
}

// Pair prices with API IDs in the same comparison table, not marketing names.
// A future family needs no code update as long as the documented shape holds.
fn claude_rates(body: &[u8]) -> Result<BTreeMap<String, (f64, f64)>, String> {
    let text = std::str::from_utf8(body).map_err(|_| "invalid Claude pricing document")?;
    let mut prices = BTreeMap::new();
    let mut table = Vec::new();
    for line in text.lines().chain(std::iter::once("")) {
        if line.trim().starts_with('|') {
            table.push(line.trim().trim_matches('|').split('|').map(str::trim).collect::<Vec<_>>());
            continue;
        }
        let row = |label: &str| table.iter().find(|r| r.first().is_some_and(|c|
            *c == label || c.strip_prefix('[').and_then(|s| s.split_once("](")).is_some_and(|(s, _)| s == label)));
        if let (Some(ids), Some(costs)) = (row("Claude API ID"), row("Pricing")) {
            if ids.len() != costs.len() { return Err("Claude pricing column mismatch".into()); }
            for (id, cost) in ids.iter().skip(1).zip(costs.iter().skip(1)) {
                let Some(id) = id.strip_prefix('`').and_then(|s| s.strip_suffix('`')).filter(|s| identifier(s)) else { continue };
                let Some((input, output)) = cost.split_once(", ") else { continue };
                let rate = input.strip_suffix(" / input MTok").and_then(dollars)
                    .zip(output.strip_suffix(" / output MTok").and_then(dollars));
                if let Some(rate) = rate {
                    if prices.insert(id.into(), rate).is_some() { return Err("duplicate Claude model price".into()); }
                }
            }
        }
        table.clear();
    }
    if prices.is_empty() { return Err("no supported Claude standard prices found".into()); }
    Ok(prices)
}

type Prices = BTreeMap<(Provider, String), (f64, f64)>;

fn source_url(provider: Provider) -> &'static str {
    match provider {
        Provider::Codex => PRICE_URL,
        Provider::Claude => crate::metadata::source_url(provider),
    }
}

fn evidence(entries: &[Entry], details: &Value, prices: &Prices) -> Value {
    let rates: Vec<_> = entries.iter().map(|e| {
        let id = crate::model_policy_ai::resolved(e, details);
        let id = if e.provider == Provider::Claude { id.strip_suffix("[1m]").unwrap_or(id) } else { id };
        (id, prices.get(&(e.provider, id.to_string())))
    }).collect();
    let mut ordered: Vec<_> = rates.iter().filter_map(|(_, rate)| rate.copied()).collect();
    ordered.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)));
    ordered.dedup();
    // Crossing input/output prices cannot establish a single cost order.
    let comparable = ordered.windows(2).all(|pair| pair[0].1 <= pair[1].1);
    json!(entries.iter().zip(rates).map(|(e, (id, rate))| {
        let preference = rate.filter(|_| comparable).and_then(|rate| ordered.iter().position(|v| v == rate))
            .map(|rank| (rank as u32 + 1) * 10);
        json!({"provider":e.provider,"model":e.model,"resolved_id":id,"relative_cost_preference":preference,
            "basis":if preference.is_some() {"api_standard_short_context_rank"} else {"unknown"},
            "source_url":rate.map(|_| source_url(e.provider)),
            "input_per_million":rate.map(|r|r.0),"output_per_million":rate.map(|r|r.1),
            "currency":rate.map(|_|"USD")})
    }).collect::<Vec<_>>())
}

pub(crate) fn research(entries: &[Entry], details: &Value, enabled: bool) -> (Value, Option<String>) {
    research_with(entries, details, enabled, &CurlFetch { timeout_secs:10 })
}

fn research_with(entries: &[Entry], details: &Value, enabled: bool, fetch: &dyn Fetch) -> (Value, Option<String>) {
    let mut prices = Prices::new();
    let mut errors = Vec::new();
    for provider in [Provider::Codex, Provider::Claude].into_iter()
        .filter(|p| enabled && entries.iter().any(|e| e.provider == *p)) {
        let result = match provider {
            Provider::Codex => retrieve(fetch, PRICE_URL, None, None).and_then(|doc| match doc {
                Document::Fresh { body, .. } => rates(&body),
                Document::NotModified => Err("pricing response had no body".into()),
            }),
            // Reuse the fresh, allowlisted overview already fetched for this draft.
            Provider::Claude => details["official_sources"].as_array().into_iter().flatten()
                .find(|s| s["provider"] == "claude" && s["status"] == "fresh" && s["url"] == source_url(provider))
                .and_then(|s| s["document"].as_str()).ok_or_else(|| "fresh Claude pricing source unavailable".to_string())
                .and_then(|doc| claude_rates(doc.as_bytes())),
        };
        match result {
            Ok(rates) => prices.extend(rates.into_iter().map(|(id, rate)| ((provider, id), rate))),
            Err(error) => errors.push(format!("{}: {error}", provider.name())),
        }
    }
    let warning = (!errors.is_empty()).then(|| format!("Could not verify cost preferences: {}. Unknown costs stay null.", errors.join("; ")));
    (evidence(entries, details, &prices), warning)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalogue::Policy;
    const CLAUDE: &str = "| Feature | Fable | Opus | Sonnet | Haiku |\n| --- | --- | --- | --- | --- |\n| [Pricing](https://platform.claude.com/docs/en/about-claude/pricing) | $10 / input MTok, $50 / output MTok | $5 / input MTok, $25 / output MTok | $2 / input MTok, $10 / output MTok | $1 / input MTok, $5 / output MTok |\n| Claude API ID | `claude-fable-5-1` | `claude-opus-5` | `claude-sonnet-5` | `claude-haiku-4-5-20251001` |\n";

    fn claude_fixture() -> (Vec<Entry>, Value) {
        let models = [("claude-fable-5-1[1m]", "claude-fable-5-1"), ("opus[1m]", "claude-opus-5[1m]"),
            ("sonnet", "claude-sonnet-5"), ("haiku", "claude-haiku-4-5-20251001"), ("unresolved", "unresolved")];
        let entries = models.iter().map(|(id, _)| json!({"provider":"claude","model":id,"tier":"standard"})).collect::<Vec<_>>();
        let mut policy = json!(Policy::default());
        policy["entries"] = json!(entries);
        let entries = Policy::from_settings(&json!({"model_catalogue":policy})).unwrap().entries;
        let details = json!({"providers":[{"provider":"claude","models":models.map(|(id,target)|
            json!({"id":id,"resolved_id":target}))}],
            "official_sources":[{"provider":"claude","url":source_url(Provider::Claude),"status":"fresh","document":CLAUDE}]});
        (entries, details)
    }

    struct Unavailable;
    impl Fetch for Unavailable {
        fn fetch(&self, _: &crate::metadata::FetchRequest) -> Result<crate::metadata::FetchResponse, String> {
            Err("pricing offline".into())
        }
    }

    #[test]
    #[ignore = "fetches live official pricing documents; run explicitly"]
    fn live_official_documents_supply_prices_for_both_providers() {
        let fetch = CurlFetch { timeout_secs:10 };
        let Document::Fresh { body, .. } = retrieve(&fetch, source_url(Provider::Claude), None, None).unwrap() else {
            panic!("expected fresh Claude document")
        };
        let claude = claude_rates(&body).unwrap();
        let Document::Fresh { body: openai, .. } = retrieve(&fetch, PRICE_URL, None, None).unwrap() else {
            panic!("expected fresh OpenAI document")
        };
        let codex = rates(&openai).unwrap();
        let mut policy = json!(Policy::default());
        policy["entries"] = json!([(Provider::Claude, claude), (Provider::Codex, codex)].into_iter()
            .flat_map(|(provider, prices)| prices.into_keys().map(move |id|
                json!({"provider":provider,"model":id,"tier":"standard"}))).collect::<Vec<_>>());
        let entries = Policy::from_settings(&json!({"model_catalogue":policy})).unwrap().entries;
        let details = json!({"official_sources":[{"provider":"claude","status":"fresh",
            "url":source_url(Provider::Claude),"document":String::from_utf8(body).unwrap()}]});
        let (result, warning) = research_with(&entries, &details, true, &fetch);
        assert!(warning.is_none(), "{warning:?}");
        for cost in result.as_array().unwrap() {
            assert!(cost["input_per_million"].is_number(), "{cost}");
            assert!(cost["output_per_million"].is_number(), "{cost}");
        }
    }

    #[test]
    fn claude_discovery_aliases_get_current_published_prices_and_unknowns_stay_null() {
        let (entries, details) = claude_fixture();
        let (result, warning) = research_with(&entries, &details, true, &Unavailable);
        assert!(warning.is_none());
        for (i, rank) in [40,30,20,10].into_iter().enumerate() {
            assert_eq!(result[i]["relative_cost_preference"], rank);
            assert_eq!(result[i]["source_url"], source_url(Provider::Claude));
        }
        assert_eq!(result[0]["input_per_million"], 10.0);
        assert_eq!(result[3]["output_per_million"], 5.0);
        assert!(result[4]["relative_cost_preference"].is_null());
        // Without discovery, a moving CLI alias must never be guessed from its name.
        assert!(research_with(&entries, &json!({"official_sources":details["official_sources"]}), true, &Unavailable).0[1]["relative_cost_preference"].is_null());
    }

    #[test]
    fn claude_parser_supports_new_families_but_rejects_other_units_and_unpaired_tables() {
        let future = CLAUDE.replace("claude-fable-5-1", "claude-future-7");
        assert_eq!(claude_rates(future.as_bytes()).unwrap()["claude-future-7"], (10.0,50.0));
        assert!(claude_rates(CLAUDE.replace("MTok", "KTok").as_bytes()).is_err());
        assert!(claude_rates(CLAUDE.replace("| Claude API ID", "\n| Claude API ID").as_bytes()).is_err());
        assert!(claude_rates(CLAUDE.replace("[Pricing]", "[Batch pricing]").as_bytes()).is_err());
        assert!(claude_rates(format!("{CLAUDE}\n{CLAUDE}").as_bytes()).is_err());
    }

    #[test]
    fn provider_failures_are_isolated_and_disabled_or_stale_research_cannot_supply_prices() {
        let (mut entries, mut details) = claude_fixture();
        let mut codex = entries[0].clone();
        codex.provider = Provider::Codex;
        codex.model = "gpt-6-astra".into();
        entries.push(codex);
        let (result, warning) = research_with(&entries, &details, true, &Unavailable);
        assert_eq!(result[3]["relative_cost_preference"],10);
        assert!(result[5]["relative_cost_preference"].is_null());
        assert!(warning.unwrap().contains("codex: pricing offline"));
        assert!(research_with(&entries, &details, false, &Unavailable).0.as_array().unwrap()
            .iter().all(|e| e["relative_cost_preference"].is_null()));
        details["official_sources"][0]["status"] = json!("stale");
        assert!(research_with(&entries, &details, true, &Unavailable).0.as_array().unwrap()
            .iter().all(|e| e["relative_cost_preference"].is_null()));
    }

    #[test]
    fn both_providers_share_one_cost_scale_and_equal_prices_tie() {
        let (mut entries, details) = claude_fixture();
        let mut codex = entries[0].clone();
        codex.provider = Provider::Codex;
        codex.model = "gpt-6-astra".into();
        entries.push(codex);
        let mut prices: Prices = claude_rates(CLAUDE.as_bytes()).unwrap().into_iter()
            .map(|(id, rate)| ((Provider::Claude, id), rate)).collect();
        prices.insert((Provider::Codex,"gpt-6-astra".into()), (10.0,50.0));
        let result = evidence(&entries, &details, &prices);
        assert_eq!(result[0]["relative_cost_preference"],result[5]["relative_cost_preference"]);
        assert_eq!(result[3]["relative_cost_preference"],10);
    }
    #[test]
    fn actual_pricing_shape_orders_luna_terra_sol_without_using_effort_or_batch_rates() {
        let doc = b"Prices per 1M tokens.\n### Standard pricing data\n\n| Model | Short context input | Short context cached input | Short context cache writes | Short context output |\n| --- | --- | --- | --- | --- |\n| gpt-5.6-sol | $4.00 | $0.40 | $5.00 | $20.00 |\n| gpt-5.6-luna | $0.20 | $0.02 | $0.25 | $1.20 |\n| gpt-5.6-terra | $2.00 | $0.20 | $2.50 | $12.00 |\n\n### Batch pricing data\n| Model | Short context input | Short context output |\n| gpt-5.6-sol | $0.01 | $0.01 |\n";
        let mut policy = json!(Policy::default());
        policy["entries"] = json!(["gpt-5.6-luna","gpt-5.6-sol","gpt-5.6-terra","unknown"].map(|model|
            json!({"provider":"codex","model":model,"tier":"standard"})));
        let entries = Policy::from_settings(&json!({"model_catalogue":policy})).unwrap().entries;
        let prices: Prices = rates(doc).unwrap().into_iter().map(|(id, rate)| ((Provider::Codex, id), rate)).collect();
        let result = evidence(&entries,&json!({}),&prices);
        assert_eq!(result[0]["relative_cost_preference"],10);
        assert_eq!(result[1]["relative_cost_preference"],30);
        assert_eq!(result[2]["relative_cost_preference"],20);
        assert!(result[3]["relative_cost_preference"].is_null());
        let mut crossed = prices;
        crossed.insert((Provider::Codex,"gpt-5.6-luna".into()),(0.2,100.0));
        assert!(evidence(&entries,&json!({}),&crossed).as_array().unwrap().iter().all(|r|r["relative_cost_preference"].is_null()));
    }
    #[test]
    fn unrecognized_units_and_only_batch_prices_never_supply_an_order() {
        assert!(rates(b"### Standard pricing data\n| Model | Input | Output |\n| model | $1 | $2 |\n").is_err());
        assert!(rates(b"Prices per 1M tokens.\n### Batch pricing data\n").is_err());
    }
}
