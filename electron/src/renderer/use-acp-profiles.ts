import { useCallback, useEffect, useState } from "react";
import type { AcpProfile, IfaceApi } from "../shared/ipc";
import { ipcMessage } from "./ipc-message";

type AcpProfilesApi = Pick<IfaceApi, "getAcpProfiles" | "setAcpProfiles">;

/** The user's ACP profiles as the settings editor sees them; Rust owns the store. */
export function useAcpProfiles(api: AcpProfilesApi) {
  const [acpProfiles, setAcpProfiles] = useState<AcpProfile[]>([]);
  const [acpProfilesPending, setAcpProfilesPending] = useState(false);
  const [acpProfilesError, setAcpProfilesError] = useState<string | null>(null);
  const [acpProfilesRestartRequired, setAcpProfilesRestartRequired] = useState(false);
  const [acpProfilesSavedRevision, setAcpProfilesSavedRevision] = useState(0);

  useEffect(() => {
    let active = true;
    api.getAcpProfiles().then(profiles => {
      if (active) setAcpProfiles(profiles);
    }, (err: unknown) => {
      if (active) setAcpProfilesError(ipcMessage(err));
    });
    return () => { active = false; };
  }, [api]);

  const saveAcpProfiles = useCallback((profiles: AcpProfile[]): void => {
    if (acpProfilesPending) return;
    setAcpProfilesPending(true);
    setAcpProfilesError(null);
    void api.setAcpProfiles(profiles).then(result => {
      setAcpProfiles(result.profiles);
      setAcpProfilesRestartRequired(result.restartRequired);
      setAcpProfilesSavedRevision(revision => revision + 1);
    }, (err: unknown) => {
      setAcpProfilesError(ipcMessage(err));
    }).finally(() => { setAcpProfilesPending(false); });
  }, [acpProfilesPending, api]);

  return {
    acpProfiles, acpProfilesPending, acpProfilesError, acpProfilesRestartRequired,
    acpProfilesSavedRevision, saveAcpProfiles,
  };
}
