// An agent-like request handled by Node.js in a cell (spec `workloads`,
// change `add-node-workload`): warm up, say "warm" and pause briefly (a host
// may freeze the cell there as a template), then answer one tool call:
// parse its JSON, run regular expressions over its text, fold it through
// closures, and reply in JSON. Given a model server's URL, it then asks the
// model over HTTP (`node:http`) and prints the tool call it got back.
// Deterministic: the same output on Linux and in a cell.
"use strict";
const http = require("node:http");

const WORDS = /[a-z]+/g;
const EMAIL = /\b([a-z]+)@([a-z]+\d)\.example\b/g;
const DATE = /\b(\d{4})-(\d{2})-(\d{2})\b/g;
const VOCAB = ["cell", "fork", "agent", "kernel", "snapshot", "resume", "page", "file"];

// A seeded generator (mulberry32): `Math.random` cannot be seeded.
function rng(seed) {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = Math.imul(a ^ (a >>> 15), 1 | a);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

function toolCall(seed) {
  const next = rng(seed);
  const pick = (n) => Math.floor(next() * n);
  const lines = [];
  for (let i = 0; i < 40; i++) {
    const words = Array.from({ length: 12 }, () => VOCAB[pick(8)]).join(" ");
    const day = new Date(Date.UTC(2026, 0, 1 + pick(365))).toISOString().slice(0, 10);
    lines.push(`${words}, ask ${VOCAB[i % 8]}@team${i % 3}.example by ${day}`);
  }
  return JSON.stringify({
    tool: "summarize",
    arguments: { document: lines.join("\n"), top: 3, since: "2026-06-01" },
  });
}

// Closures over closures: small functions composed, and a counter closure
// per word, run over every word.
function pipeline(words) {
  const stages = [
    (w) => w.toUpperCase(),
    (w) => w.split("").reverse().join(""),
    (w) => w.length,
  ];
  const compose = (fs) => fs.reduce((f, g) => (x) => g(f(x)), (x) => x);
  const counters = new Map();
  const counter = (key) => {
    if (!counters.has(key)) {
      let n = 0;
      counters.set(key, () => ++n);
    }
    return counters.get(key);
  };
  const length = compose(stages);
  let total = 0;
  for (let round = 0; round < 4; round++) {
    for (const w of words) {
      total += length(w) * counter(w)();
    }
  }
  return total;
}

function answer(request) {
  const call = JSON.parse(request);
  const args = call.arguments;
  const text = args.document;
  const words = text.match(WORDS);
  const freq = {};
  for (const w of words) {
    if (VOCAB.includes(w)) freq[w] = (freq[w] || 0) + 1;
  }
  const top = Object.entries(freq)
    .sort((a, b) => b[1] - a[1] || (a[0] < b[0] ? -1 : 1))
    .slice(0, args.top);
  const contacts = [...new Set([...text.matchAll(EMAIL)].map((m) => `${m[1]}@${m[2]}`))].sort();
  const dates = [...text.matchAll(DATE)].map((m) => m[0]).filter((d) => d >= args.since).sort();
  return JSON.stringify({
    tool: call.tool,
    top_words: top.map(([w]) => w.toUpperCase()),
    counts: top.map(([, n]) => n),
    contacts: contacts.length,
    first_contact: contacts[0],
    dates_since: dates.length,
    last_date: dates[dates.length - 1],
    folded: pipeline(words),
  });
}

// Asks the model at `url` and resolves with the tool call it answers.
function ask(url, reply) {
  const body = JSON.stringify({
    model: "rhizo-fake",
    messages: [{ role: "user", content: reply }],
  });
  return new Promise((resolve, reject) => {
    const req = http.request(
      url + "/v1/chat/completions",
      {
        method: "POST",
        headers: { "Content-Type": "application/json", "Content-Length": Buffer.byteLength(body) },
        agent: false,
      },
      (res) => {
        let data = "";
        res.setEncoding("utf8");
        res.on("data", (chunk) => (data += chunk));
        res.on("end", () => {
          const fn = JSON.parse(data).choices[0].message.tool_calls[0].function;
          resolve(`${res.statusCode} ${fn.name} ${fn.arguments}`);
        });
      },
    );
    req.on("error", reject);
    req.end(body);
  });
}

async function main() {
  answer(toolCall(1)); // warm-up: every path once (what a template keeps)
  const request = toolCall(42);
  console.log("warm");
  await new Promise((resolve) => setTimeout(resolve, 50));
  const reply = answer(request);
  console.log(reply);
  const url = process.argv[2];
  if (url) {
    console.log(`model: ${await ask(url, reply)}`);
  }
}

main();
