// Transport only, loaded into a private one-shot OpenCode CLI process.
// OpenCode supplies authentication; Rust owns quota interpretation and refresh.
const PREFIX = "INTERFACE_OPENCODE_GO_USAGE_V1 ";
const emit = frame => process.stdout.write(`${PREFIX}${JSON.stringify({ version: 1, ...frame })}\n`);

export default async function () {
  emit({ type: "ready" });
  return { auth: {
    provider: "opencode-go",
    methods: [],
    loader: async getAuth => {
      try {
        const auth = await getAuth();
        if (auth?.type !== "api" || typeof auth.key !== "string" || auth.key.length === 0) {
          emit({ type: "response", status: 401 });
          return {};
        }
        const response = await fetch("https://opencode.ai/zen/go/v1/usage", {
          method: "GET",
          headers: { Authorization: `Bearer ${auth.key}`, Accept: "application/json",
            "User-Agent": "Interface/OpenCode-Go-Usage" },
          redirect: "error",
          signal: AbortSignal.timeout(10000),
        });
        // Error bodies may contain account metadata. Only the HTTP status is
        // needed to explain an authentication or entitlement failure.
        if (response.status !== 200) {
          await response.body?.cancel();
          emit({ type: "response", status: response.status });
          return {};
        }
        const reader = response.body?.getReader();
        if (!reader) throw new Error("missing body");
        const chunks = [];
        let size = 0;
        try {
          for (;;) {
            const part = await reader.read();
            if (part.done) break;
            size += part.value.byteLength;
            if (size > 8192) throw new Error("oversized body");
            chunks.push(part.value);
          }
        } finally {
          await reader.cancel().catch(() => {});
        }
        const bytes = new Uint8Array(size);
        let offset = 0;
        for (const chunk of chunks) { bytes.set(chunk, offset); offset += chunk.byteLength; }
        const body = JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(bytes));
        // Copy only the public quota fields across the process boundary.
        const usage = Object.fromEntries(["rolling", "weekly", "monthly"].map(name => {
          const window = body?.usage?.[name];
          return [name, { status: window?.status, percent: window?.percent, resetsAt: window?.resetsAt }];
        }));
        emit({ type: "response", status: 200, body: { usage } });
      } catch {
        emit({ type: "response", status: 0 });
      }
      // No model call, auth mutation, provider reconfiguration or local store.
      return {};
    },
  } };
}
