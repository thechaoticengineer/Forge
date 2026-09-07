#!/usr/bin/env node
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { createRequire } from 'node:module';
import { discover } from './discovery.mjs';
const [protocol, executable, cliVersion] = process.argv.slice(2);
const envelope = { bridge_version: 1, source: 'claude_code_initialization', cli_version: cliVersion, sdk_version: '0.3.261' };
try {
  if (protocol !== '--forge-discovery-v1' || executable !== 'claude' || cliVersion !== '2.1.261 (Claude Code)')
    throw new Error('unsupported CLI/bridge version');
  const require = createRequire(import.meta.url);
  if (JSON.parse(readFileSync(join(dirname(require.resolve('@anthropic-ai/claude-agent-sdk')), 'package.json'), 'utf8')).version !== envelope.sdk_version)
    throw new Error('unsupported SDK version');
  const { query } = await import('@anthropic-ai/claude-agent-sdk');
  const models = await discover(query, executable);
  process.stdout.write(JSON.stringify({ ...envelope, models }) + '\n');
} catch (e) {
  // Never emit account info, raw SDK errors, headers, or credential material.
  const auth = /unauthorized|authentication_error|authentication failed|not logged in|invalid.api.key|token expired/i.test(String(e));
  const unsupported = /unsupported|cannot find (module|package)/i.test(String(e));
  process.stdout.write(JSON.stringify({ ...envelope, error: { code: auth ? 401 : unsupported ? -32601 : -32000,
    message: auth ? 'authentication failed' : unsupported ? 'unsupported discovery' : 'discovery failed' } }) + '\n');
  process.exitCode = 1;
}
