import type { InjectionKey } from "vue";

export const terminalPluginToolsKey: InjectionKey<(contextKey: string) => void> = Symbol("terminalPluginTools");
