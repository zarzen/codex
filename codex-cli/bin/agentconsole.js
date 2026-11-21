#!/usr/bin/env node
import { spawn } from "node:child_process";
import { createInterface } from "node:readline";
import { createServer } from "node:net";
import {
  existsSync,
  mkdtempSync,
  readFileSync,
  renameSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { randomBytes } from "node:crypto";
import { once } from "node:events";
import { fileURLToPath } from "node:url";

const help =
  "agentconsole CONFIG.json\nSet main.model and components.<name>.model in the JSON configuration. Omitted models use the app-server default.\nSet top-level effort to none, minimal, low, medium, high, xhigh, max, or ultra for all agents, including observers. Override it with main.effort or components.<name>.effort. Omitted effort uses existing defaults.\nClick a system agent to attach. Press Ctrl+X, release it, then press N for the next agent, T for Trace, or Q for Queue. Esc returns to chat.\nSelect chat text and press Ctrl+X then O to discuss it in an observer, or press d on a trace selection. Ctrl+X then O in an observer returns to its source.\nAn observer inherits the displayed model unless observer.model is set.";
if (process.argv.length !== 3 || process.argv[2] === "--help") {
  console.log(help);
  process.exit(process.argv[2] === "--help" ? 0 : 2);
}
if (typeof WebSocket === "undefined")
  throw Error("AgentConsole requires Node.js 22 or newer");
if (!process.stdin.isTTY || !process.stdout.isTTY)
  throw Error("AgentConsole requires an interactive terminal");
const config = JSON.parse(readFileSync(process.argv[2], "utf8"));
function validateEffort(value, path) {
  if (
    value !== undefined &&
    !["none", "minimal", "low", "medium", "high", "xhigh", "max", "ultra"].includes(
      value,
    )
  )
    throw Error(path + " must be none, minimal, low, medium, high, xhigh, max, or ultra");
}
validateEffort(config.effort, "effort");
const effortArgs =
  config.effort === undefined
    ? []
    : ["-c", "model_reasoning_effort=" + JSON.stringify(config.effort)];
const workers = Object.keys(config.components);
if (
  !workers.length ||
  workers.some((n) => !/^[\w-]+$/.test(n) || n === "main")
) {
  throw new Error(
    "Use unique component names containing letters, numbers, _ or -, excluding main.",
  );
}
for (const [path, settings] of [
  ["main", config.main],
  ["observer", config.observer],
  ...workers.map((name) => ["components." + name, config.components[name]]),
]) {
  if (settings === undefined) continue;
  if (!settings || typeof settings !== "object" || Array.isArray(settings))
    throw Error(path + " must be an object");
  if (
    settings.model !== undefined &&
    (typeof settings.model !== "string" || !settings.model.trim())
  )
    throw Error(path + ".model must be a non-empty model ID");
  if (path !== "observer") validateEffort(settings.effort, path + ".effort");
}
const debugBinary = fileURLToPath(
  new URL("../../codex-rs/target/debug/codex", import.meta.url),
);
const packageBinary = fileURLToPath(new URL("./codex.js", import.meta.url));
const binary =
  process.env.CODEX_BIN ||
  (existsSync(debugBinary) ? debugBinary : packageBinary);
if (!existsSync(binary))
  throw Error(
    "Build the native client with cargo build -p codex-cli, or set CODEX_BIN to the absolute path of the fork binary.",
  );
const directory = mkdtempSync(
  join(process.platform === "darwin" ? "/tmp" : tmpdir(), "agentconsole-"),
);
const token = randomBytes(32).toString("hex");
const tokenFile = join(directory, "token");
writeFileSync(tokenFile, token, { mode: 0o600 });
const control = join(directory, "console.sock");
const daemon = spawn(
  binary,
  [
    ...effortArgs,
    "app-server",
    "--listen",
    "ws://127.0.0.1:0",
    "--ws-auth",
    "capability-token",
    "--ws-token-file",
    tokenFile,
  ],
  { stdio: ["ignore", "ignore", "pipe"] },
);
let client;
for (const signal of ["SIGHUP", "SIGTERM"])
  process.once(signal, () => process.exit(1));
process.on("exit", () => {
  client?.kill();
  daemon.kill();
  rmSync(directory, { recursive: true, force: true });
});
const logs = createInterface({ input: daemon.stderr });
let endpoint;
for await (const line of logs) {
  endpoint = /listening on:\s*(ws:\/\/127\.0\.0\.1:\d+)/.exec(line)?.[1];
  if (endpoint) break;
}
if (!endpoint) throw Error("Private app-server failed to publish its endpoint");
daemon.stderr.resume();
const server = new WebSocket(endpoint, {
  headers: { Authorization: "Bearer " + token },
});
await once(server, "open");
const pending = new Map();
const agents = new Map();
const byThread = new Map();
let serial = 0,
  taskId = 0;
function publishStatus() {
  const state = Object.fromEntries(
    [...agents.values()].map((agent) => [
      agent.id,
      {
        active: Boolean(agent.active || agent.liveTurn),
        waiting: agent.queue.length,
      },
    ]),
  );
  const temporary = join(directory, "status.tmp");
  writeFileSync(temporary, JSON.stringify(state));
  renameSync(temporary, join(directory, "status.json"));
}

function send(packet) {
  server.send(JSON.stringify(packet));
}
function rpc(method, params) {
  const id = ++serial;
  return new Promise((resolve, reject) => {
    pending.set(id, { resolve, reject });
    send({ id, method, params });
  });
}
function stop(error) {
  for (const p of pending.values()) p.reject(error);
  pending.clear();
  client?.kill();
}
server.addEventListener("close", () => stop(Error("app-server disconnected")));
const input = (text) => [{ type: "text", text, text_elements: [] }];
function enqueue(a, text, items = input(text)) {
  const job = { id: ++taskId, text, items, status: "queued", output: [] };
  a.jobs.push(job);
  a.queue.push(job);
  publishStatus();
  const result = new Promise((resolve) => {
    job.resolve = resolve;
  });
  pump(a);
  return result;
}
async function pump(a) {
  if (a.active || a.liveTurn || !a.queue.length) return;
  const job = (a.active = a.queue.shift());
  job.status = "running";
  publishStatus();

  try {
    const { turn } = await rpc("turn/start", {
      threadId: a.id,
      input: job.items,
    });
    if (a.active === job) job.turn = turn.id;
  } catch (e) {
    if (a.active === job) finish(a, "failed", e.message);
  }
}
function finish(a, status, detail) {
  const job = a.active;
  if (!job) return;
  job.status = status;
  if (detail) job.output.push(detail);
  job.resolve({ id: job.id, status, output: job.output.join("\n") });
  a.active = null;
  publishStatus();
  if (detail) a.trace.push({ type: "error", text: detail });

  pump(a);
}
async function receive(m) {
  if (!m.method) {
    const call = pending.get(m.id);
    if (!call) return;
    pending.delete(m.id);
    if (m.error) call.reject(new Error(m.error.message));
    else call.resolve(m.result);
    return;
  }
  const p = m.params || {},
    a = byThread.get(p.threadId);
  if (m.id !== undefined) {
    if (
      m.method === "item/tool/call" &&
      a?.name === "main" &&
      p.tool === "delegate_task"
    ) {
      const { component, task } = p.arguments || {};
      if (
        workers.includes(component) &&
        typeof task === "string" &&
        task.trim()
      ) {
        const result = await enqueue(agents.get(component), task);
        send({
          id: m.id,
          result: {
            success: result.status === "completed",
            contentItems: [{ type: "inputText", text: JSON.stringify(result) }],
          },
        });
        return;
      }
      send({
        id: m.id,
        result: {
          success: false,
          contentItems: [
            {
              type: "inputText",
              text: "Expected a configured component and a non-empty task.",
            },
          ],
        },
      });
    }
    // The native TUI owns approvals and other interactive requests.
    return;
  }
  if (!a) return;
  if (m.method === "item/started" || m.method === "item/completed") {
    const i = p.item.id
      ? a.trace.findIndex((item) => item.id === p.item.id)
      : -1;
    if (i < 0) a.trace.push(p.item);
    else a.trace[i] = p.item;
    if (m.method === "item/completed" && p.item.type === "agentMessage")
      a.active?.output.push(p.item.text);
  }
  if (m.method === "item/agentMessage/delta") {
    const item = a.trace.find((i) => i.id === p.itemId);
    if (item) item.text = (item.text || "") + p.delta;
  }
  if (m.method === "turn/started") {
    a.liveTurn = p.turn.id;
    if (a.active) a.active.turn = p.turn.id;
  }
  if (m.method === "turn/completed" && a.liveTurn === p.turn.id)
    a.liveTurn = null;
  if (
    m.method === "turn/completed" &&
    a.active &&
    (!a.active.turn || a.active.turn === p.turn.id)
  ) {
    finish(a, p.turn.status, p.turn.error?.message);
  } else if (m.method === "turn/completed") pump(a);
  if (m.method === "turn/started" || m.method === "turn/completed")
    publishStatus();
}
server.addEventListener("message", (event) => {
  receive(JSON.parse(event.data)).catch((e) => {
    console.error(e.message);
  });
});
const tool = {
  type: "function",
  name: "delegate_task",
  description:
    "Queue work for a configured component and receive its result. " +
    workers.map((n) => n + ": " + config.components[n].instructions).join("\n"),
  inputSchema: {
    type: "object",
    properties: {
      component: { type: "string", enum: workers },
      task: { type: "string" },
    },
    required: ["component", "task"],
    additionalProperties: false,
  },
};
async function start(name, settings, extra = {}) {
  const effort = settings.effort ?? config.effort;
  const { thread } = await rpc("thread/start", {
    cwd: process.cwd(),
    model: settings.model?.trim(),
    config: effort === undefined ? undefined : { model_reasoning_effort: effort },
    developerInstructions: settings.instructions,
    approvalPolicy: "never",
    historyMode: "paginated",
    ...extra,
  });
  // Seed a saved history so a second client can resume this component before its first turn.
  await rpc("thread/inject_items", {
    threadId: thread.id,
    items: [
      {
        type: "message",
        role: "user",
        content: [
          { type: "input_text", text: "AgentConsole component: " + name },
        ],
      },
    ],
  });
  const a = {
    name,
    id: thread.id,
    jobs: [],
    queue: [],
    trace: [],
    active: null,
  };
  agents.set(name, a);
  byThread.set(a.id, a);
  return a;
}
await rpc("initialize", {
  clientInfo: { name: "agentconsole", version: "0.1" },
  capabilities: { experimentalApi: true },
});
send({ method: "initialized" });
for (const n of workers) await start(n, config.components[n]);
await start("main", config.main || {}, { dynamicTools: [tool] });
async function action(request) {
  const agent = byThread.get(request.id);
  if (!agent) throw Error("Choose a configured agent");
  if (request.action === "Queue")
    return (
      agent.jobs
        .map(
          (j) =>
            "#" +
            j.id +
            " " +
            j.status +
            "  " +
            j.text +
            "\n" +
            j.output.join("\n"),
        )
        .join("\n") || "No queued tasks"
    );
  if (request.action === "Trace")
    return (
      agent.trace.map((i) => JSON.stringify(i, null, 2)).join("\n") ||
      "No trace events yet"
    );
  throw Error("Unknown component action");
}
const listener = createServer((connection) => {
  connection.on("error", () => {});
  createInterface({ input: connection }).once("line", async (line) => {
    try {
      connection.end(
        JSON.stringify({ value: await action(JSON.parse(line)) }) + "\n",
      );
    } catch (e) {
      connection.end(JSON.stringify({ error: e.message }) + "\n");
    }
  });
});
listener.listen(control);
await once(listener, "listening");
publishStatus();
const rows = [agents.get("main"), ...workers.map((n) => agents.get(n))].map(
  ({ name, id }) => ({ name, id }),
);
client = spawn(
  binary,
  [
    "--remote",
    endpoint,
    "--remote-auth-token-env",
    "AGENTCONSOLE_TOKEN",
    "-c",
    "tui.fullscreen_transcript=true",
    "-c",
    'tui.alternate_screen="always"',
    "-c",
    "tui.copy_on_select_clear_selection=false",
    "resume",
    rows[0].id,
  ],
  {
    stdio: "inherit",
    env: {
      ...process.env,
      AGENTCONSOLE_TOKEN: token,
      CODEX_AGENTCONSOLE: JSON.stringify({
        control,
        rows,
        observer_model: config.observer?.model?.trim(),
      }),
    },
  },
);
const [code] = await once(client, "exit");
listener.close();
server.close();
daemon.kill();
process.exitCode = code ?? 1;
