// Protocol v1: initialization only. Never yield a user message to the SDK.
export async function discover(query, executable) {
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
    return await q.supportedModels();
  } finally { release(); q.close(); }
}
