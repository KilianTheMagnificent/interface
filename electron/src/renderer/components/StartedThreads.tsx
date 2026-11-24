import "./StartedThreads.css";
import { memo } from "react";
import { startedThreadStatusWords, type StartedThreadsLine } from "../started-threads";

/**
 * Above a thread's composer when its agent started other threads (team
 * tools, protocol 57): each as its status mark and title, newest first;
 * a title opens its thread. Rust's view says all of it.
 */
export const StartedThreads = memo(function StartedThreads({ line, onOpen }: {
  line: StartedThreadsLine;
  onOpen: (key: string) => void;
}) {
  return (
    <div className="started-threads" role="group" aria-label="Started threads">
      <span className="started-threads-label">Started threads</span>
      {line.items.map(item => {
        const words = startedThreadStatusWords(item.status);
        return (
          <button key={item.key} type="button" className="started-threads-item"
            title={`${item.title} · ${words}`} onClick={() => { onOpen(item.key); }}>
            <span className={`started-threads-mark is-${item.status}`} role="img" aria-label={words} />
            <span className="started-threads-title">{item.title}</span>
          </button>
        );
      })}
      {line.more > 0 && <span className="started-threads-more">+{line.more} more</span>}
    </div>
  );
});
