// Protocol v1: initialization only. Never yield a user message to the SDK.
export async function discover(query, executable, operation = 'models') {
  let release;
  const pending = new Promise(resolve => { release = resolve; });
  async function* noMessages() { await pending; }
  const q = query({ prompt: noMessages(), options: {
    pathToClaudeCodeExecutable: executable,
    cwd: process.cwd(), persistSession: false, tools: [],
    settingSources: ["user"], settings: { disableAllHooks: true }, mcpServers: {},
    extraArgs: { "strict-mcp-config": null },
  }});
  try {
    // This awaits the control handshake, without iterating the generation stream.
    if (operation === 'usage') {
      const result = await q.usage_EXPERIMENTAL_MAY_CHANGE_DO_NOT_RELY_ON_THIS_API_YET({ skipBehaviors: true });
      return normalizeUsage(result);
    }
    return await q.supportedModels();
  } finally { release(); q.close(); }
}

// Explicit allowlist: never forward account details, session costs or raw errors.
export function normalizeUsage(result) {
  const limits = result.rate_limits;
  const windows = [];
  function add(name, model, value) {
    if (!value) return;
    const percent = value.utilization;
    const reset = typeof value.resets_at === 'string' ? Date.parse(value.resets_at) : NaN;
    windows.push({ name, model,
      used_percent: typeof percent === 'number' && Number.isFinite(percent) && percent >= 0 && percent <= 100 ? percent : null,
      resets_at: Number.isFinite(reset) ? new Date(reset).toISOString() : null,
      resets_unix: Number.isFinite(reset) ? Math.floor(reset / 1000) : null });
  }
  if (result.rate_limits_available === true && limits) {
    add('Claude · 5h', null, limits.five_hour);
    add('Claude · weekly', null, limits.seven_day);
    add('Claude apps · weekly', null, limits.seven_day_oauth_apps);
    add('Opus · weekly', 'Opus', limits.seven_day_opus);
    add('Sonnet · weekly', 'Sonnet', limits.seven_day_sonnet);
    for (const window of (limits.model_scoped ?? []).slice(0, 32)) {
      const model = window.display_name;
      if (typeof model === 'string' && /^[a-zA-Z0-9 -]{1,64}$/.test(model))
        add(`${model} · weekly`, model, window);
    }
  }
  return { available: result.rate_limits_available === true && !!limits, windows,
    extra_usage_enabled: typeof limits?.extra_usage?.is_enabled === 'boolean' ? limits.extra_usage.is_enabled : null };
}
