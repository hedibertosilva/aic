// Spike only: read Codex quotas through its own authenticated app-server.
// No thread/start, turn/start, prompts, auth-file reads or raw stderr output.
// Usage: node docs/agent-quotas-probe.cjs [path-to-codex]
const { spawn } = require("node:child_process");
const { performance } = require("node:perf_hooks");

const started = performance.now();
const child = spawn(process.argv[2] || "codex", ["app-server", "--listen", "stdio://"], {
  stdio: ["pipe", "pipe", "pipe"],
  detached: true,
});
let buffer = "", finished = false;

function kill(signal) {
  if (!child.pid) return;
  try { process.kill(-child.pid, signal); } catch { /* already exited */ }
}
function finish(result) {
  if (finished) return;
  finished = true;
  clearTimeout(deadline);
  console.log(JSON.stringify({
    source: "codex app-server: account/rateLimits/read",
    attempted_at: new Date().toISOString(),
    elapsed_ms: Math.round(performance.now() - started),
    ...result,
  }, null, 2));
  process.exitCode = result.status === "available" ? 0 : 1;
  child.stdin.end();
  kill("SIGTERM");
  setTimeout(() => kill("SIGKILL"), 500);
}
function send(message) { child.stdin.write(JSON.stringify(message) + "\n"); }
function windowOf(window) {
  if (!window) return null;
  return {
    used_pct: window.usedPercent ?? null,
    window_minutes: window.windowDurationMins ?? null,
    resets_at: window.resetsAt ?? null,
  };
}
const deadline = setTimeout(() => finish({ status: "timeout" }), 12000);
child.stderr.resume(); // Drain without logging account details or credentials.
child.on("error", error => finish({ status: "spawn_error", code: error.code }));
child.on("exit", (code, signal) => finish({ status: "early_exit", code, signal }));
child.stdin.on("error", error => finish({ status: "stdin_error", code: error.code }));
child.stdout.on("data", chunk => {
  buffer += chunk.toString();
  if (buffer.length > 1024 * 1024) return finish({ status: "output_too_large" });
  let newline;
  while (!finished && (newline = buffer.indexOf("\n")) >= 0) {
    const line = buffer.slice(0, newline);
    buffer = buffer.slice(newline + 1);
    let message;
    try { message = JSON.parse(line); } catch { continue; }
    if (message.id !== 1 && message.id !== 2) continue;
    if (message.error) {
      // Keep only the RPC code: provider error messages can contain private data.
      finish({ status: "read_error", stage: message.id === 1 ? "initialize" : "quota", code: message.error.code });
    } else if (message.id === 1) {
      send({ method: "initialized", params: {} });
      send({ id: 2, method: "account/rateLimits/read" });
    } else {
      const result = message.result || {};
      const buckets = result.rateLimitsByLimitId || (result.rateLimits ? { default: result.rateLimits } : {});
      const windows = Object.entries(buckets).map(([id, bucket]) => ({
        id,
        primary: windowOf(bucket.primary),
        secondary: windowOf(bucket.secondary),
      }));
      const available = windows.some(bucket => [bucket.primary, bucket.secondary].some(window => window?.used_pct != null));
      finish({ status: available ? "available" : "unavailable", fetched_at: new Date().toISOString(), windows });
    }
  }
});
send({ id: 1, method: "initialize", params: { clientInfo: { name: "aic-quota-spike", version: "0.1.0" } } });
