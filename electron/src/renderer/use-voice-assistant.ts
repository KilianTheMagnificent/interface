import { useCallback, useEffect, useRef, useState } from "react";
import type { SessionKey } from "../shared/engine";
import type { IfaceApi, VoiceEvent, VoiceState } from "../shared/ipc";
import { ipcMessage } from "./ipc-message";
import { nextVoiceState, runUiCall, type VoiceUiOutcome } from "./voice-ui";

type VoiceApi = Pick<IfaceApi, "onVoice" | "onVoiceUiCall" | "voiceUiResult" | "voiceSetEnabled">;

/**
 * The Rust host owns the helper process, the voice session and every state
 * change; this holds the last frame it pushed and draws it. `null` is "the
 * host has said nothing yet", which renders exactly as off.
 */
export function useVoiceAssistant({ api, cwd, selectSession, reportError }: {
  api: VoiceApi;
  /** The project the assistant should work in. */
  cwd: string;
  selectSession: (key: SessionKey) => void;
  reportError: (message: string) => void;
}) {
  const [voice, setVoice] = useState<VoiceEvent | null>(null);
  const [voiceBusy, setVoiceBusy] = useState(false);
  const voiceState: VoiceState = voice?.state ?? "off";
  /**
   * ONCE PER SESSION, NOT ONCE PER FRAME. Every push carries the session key,
   * including the ~10 Hz level frames while it listens, so selecting on each
   * one would drag the user back from whatever they had just navigated to
   * within 100 ms, close the launch sheet under them, and clear the action
   * error they were reading. The key changing is the event; `null` (the
   * assistant going off) re-arms it, so turning it on again selects again.
   */
  const voiceSelectedRef = useRef<string | null>(null);
  const selectVoiceSession = useCallback((key: string | null) => {
    if (key === null) {
      voiceSelectedRef.current = null;
      return;
    }
    if (voiceSelectedRef.current === key) return;
    voiceSelectedRef.current = key;
    selectSession(key as SessionKey);
  }, [selectSession]);
  useEffect(() => api.onVoice(event => {
    setVoice(current => nextVoiceState(current, event));
    // An off frame still names the session it was using; it is not a
    // destination, it is the assistant shutting down.
    selectVoiceSession(event.state === "off" ? null : event.session_key);
  }), [api, selectVoiceSession]);
  // The UI tools run against the live document; the host's own 10 s timeout is
  // the only deadline, so an answer is always sent, including for a throw.
  useEffect(() => api.onVoiceUiCall(call => {
    let outcome: VoiceUiOutcome;
    try {
      outcome = runUiCall(document, call.tool, call.input);
    } catch (err: unknown) {
      outcome = { ok: false, output: ipcMessage(err) };
    }
    void api.voiceUiResult(call.call_id, outcome.ok, outcome.output)
      .catch((err: unknown) => {
        console.warn("[app] a voice UI result could not be delivered:", err);
      });
  }), [api]);
  const toggleVoice = useCallback(() => {
    if (voiceBusy) return;
    // Error is a resting state the user gets out of by asking again, so it
    // turns the assistant ON rather than off.
    const enable = voiceState === "off" || voiceState === "error";
    setVoiceBusy(true);
    api.voiceSetEnabled(enable, cwd).then(result => {
      setVoice(current => ({
        state: result.state,
        session_key: result.session_key,
        cwd: enable ? cwd : null,
        partial: null,
        level: null,
        message: result.message,
        microphone: current?.microphone ?? null,
        asset: null,
      }));
      selectVoiceSession(enable ? result.session_key : null);
    }, (err: unknown) => {
      reportError(ipcMessage(err));
    }).finally(() => { setVoiceBusy(false); });
  }, [api, cwd, reportError, selectVoiceSession, voiceBusy, voiceState]);

  return { voice, voiceState, voiceBusy, toggleVoice };
}
