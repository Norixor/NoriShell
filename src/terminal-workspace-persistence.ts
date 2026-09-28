type TerminalWorkspaceFlush = () => Promise<void>;

let activeFlush: TerminalWorkspaceFlush | null = null;
let remoteFlush: TerminalWorkspaceFlush | null = null;

/** Registers the mounted Terminal workspace's durable-write barrier. */
export function registerTerminalWorkspaceFlush(flush: TerminalWorkspaceFlush): () => void {
  activeFlush = flush;
  return () => {
    if (activeFlush === flush) activeFlush = null;
  };
}

/** The main shell waits for each independent Tab WebView before native exit. */
export function registerRemoteTerminalWorkspaceFlush(flush: TerminalWorkspaceFlush): () => void {
  remoteFlush = flush;
  return () => { if (remoteFlush === flush) remoteFlush = null; };
}

/** Resolves only after all queued layout writes and the latest projection are durable. */
export async function flushTerminalWorkspaceBeforeExit(): Promise<void> {
  await activeFlush?.();
  await remoteFlush?.();
}

/** Requests native exit only after the active workspace confirms durability. */
export async function requestExitAfterTerminalWorkspaceFlush<T>(
  requestExit: () => Promise<T>,
): Promise<T> {
  await flushTerminalWorkspaceBeforeExit();
  return await requestExit();
}

export function resetTerminalWorkspaceFlushForTests() {
  activeFlush = null;
  remoteFlush = null;
}
