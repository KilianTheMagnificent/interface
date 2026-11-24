import type { EngineId, ModelOption } from "../shared/engine";

// Cold-render picker labels only. Provider discovery from the Rust host remains
// authoritative as soon as it arrives; the renderer owns no executable or
// provider capability decision here.
const GROK_MODELS: readonly ModelOption[] = [
  {
    value: "grok-4.6",
    displayName: "Grok 4.6",
    description: "SpaceXAI's latest frontier model",
    supportsEffort: true,
    effortLevels: ["low", "medium", "high", "xhigh"],
  },
  {
    value: "grok-4.5",
    displayName: "Grok 4.5",
    supportsEffort: true,
    effortLevels: ["low", "medium", "high"],
  },
];

const QWEN_MODELS: readonly ModelOption[] = [
  {
    value: "mlx-community/Qwen3.8-27B-4bit",
    displayName: "Qwen 3.8 27B",
    description: "Local MLX 4-bit, tuned for 48 GB Macs",
    supportsEffort: false,
    effortLevels: [],
  },
];

/** Presentation fallback shown only before a provider catalogue is available. */
export function builtinModelFallback(engine: EngineId): ModelOption[] {
  const models = engine === "acp:grok" ? GROK_MODELS
    : engine === "acp:qwen" ? QWEN_MODELS
    : [];
  return models.map(model => ({ ...model, effortLevels: [...model.effortLevels] }));
}
