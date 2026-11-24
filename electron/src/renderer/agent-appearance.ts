import type { AgentColor } from "./agent-profiles";

/** Vector characters keyed by the existing saved colour; identity survives reloads. */
export const AGENT_CHARACTERS: Record<AgentColor, { name: string; color: string; path: string }> = {
  teal: { name: "Teal droplet", color: "#479b91",
    path: "M32 3C28 3 9 26 7 38C4 53 16 61 31 61C47 61 59 51 56 37C54 25 37 3 32 3Z" },
  coral: { name: "Coral blossom", color: "#e57989",
    path: "M32 6C41 1 49 8 49 17C60 17 65 29 56 37C62 48 54 59 43 56C38 65 25 65 21 55C9 60 1 49 7 39C-1 31 4 18 15 18C14 8 23 1 32 6Z" },
  sky: { name: "Blue pebble", color: "#629ed4",
    path: "M12 10C25 1 44 4 53 17C63 29 62 47 50 56C37 64 17 61 8 49C0 37 1 19 12 10Z" },
  sand: { name: "Amber triangle", color: "#d39b45",
    path: "M25 8C28 2 35 2 39 8L61 49C65 56 61 61 54 61H10C3 61 0 56 4 49Z" },
  violet: { name: "Violet diamond", color: "#9d82c9",
    path: "M25 5C29 1 35 1 39 5L59 25C64 29 64 35 59 40L39 59C35 64 29 64 25 59L5 40C0 35 0 29 5 25Z" },
  moss: { name: "Green arch", color: "#899c5d",
    path: "M5 31C5 14 16 3 32 3C48 3 59 14 59 31V51C59 57 55 61 49 61H15C9 61 5 57 5 51Z" },
};

export const LEGACY_AGENT_SHAPES = { teal: "droplet", coral: "blossom", sky: "pebble", sand: "triangle", violet: "diamond", moss: "arch" } as const;
export const AGENT_SHAPE_PATHS: Record<import("../shared/agents").AgentShape, string> = {
  droplet: AGENT_CHARACTERS.teal.path, blossom: AGENT_CHARACTERS.coral.path,
  pebble: AGENT_CHARACTERS.sky.path, triangle: AGENT_CHARACTERS.sand.path,
  diamond: AGENT_CHARACTERS.violet.path, arch: AGENT_CHARACTERS.moss.path,
  // Star and hexagon round their corners like the six original characters.
  star: "M34.7 7.4L39.7 17.3Q41 20 44 20.4L55.1 22.1Q61 23 56.8 27.2L48.1 35.9Q46 38 46.4 41L48.2 53.1Q49 59 43.8 56L34.6 50.5Q32 49 29.4 50.5L19.2 56.1Q14 59 15.1 53.1L17.4 40.9Q18 38 15.9 35.9L7.2 27.2Q3 23 8.9 22.1L20 20.4Q23 20 24.3 17.3L29.3 7.4Q32 2 34.7 7.4Z",
  hexagon: "M25 5L39 5Q46 5 49.4 11.1L57.6 25.9Q61 32 57.6 38.1L49.4 52.9Q46 59 39 59L25 59Q18 59 14.6 52.9L6.4 38.1Q3 32 6.4 25.9L14.6 11.1Q18 5 25 5Z",
  square: "M15 4H49Q60 4 60 15V49Q60 60 49 60H15Q4 60 4 49V15Q4 4 15 4Z",
};
