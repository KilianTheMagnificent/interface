import "./ProjectHue.css";
import { diffstatBlocks, fileBadgeFor, projectHueClass, projectInitial } from "../project-hue";

/** A project's letter tile in its hue; a plain chat's is a grey dot. */
export function ProjectTile({ root, name, large }: {
  root: string | null | undefined;
  name: string | null | undefined;
  large?: boolean;
}) {
  return (
    <span className={`project-tile ${projectHueClass(root)}${large === true ? " is-large" : ""}`}
      aria-hidden="true">
      {root === null || root === undefined || root.trim() === "" ? "·" : projectInitial(name)}
    </span>
  );
}

/** A file's type badge: "TS", "RS", "MD"… in its language's colour. */
export function FileBadge({ path }: { path: string }) {
  const badge = fileBadgeFor(path);
  return <span className={`file-badge kind-${badge.kind}`} aria-hidden="true">{badge.label}</span>;
}

/** Five squares for how a change splits between added and removed lines. */
export function DiffstatBar({ added, removed }: { added: number; removed: number }) {
  return (
    <span className="diffstat-bar" aria-hidden="true">
      {diffstatBlocks(added, removed).map((block, index) => (
        <i key={index} className={block === "neutral" ? undefined : `is-${block}`} />
      ))}
    </span>
  );
}
