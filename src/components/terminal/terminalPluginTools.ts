import type { InjectionKey, Ref } from "vue";

export const terminalPluginToolsKey: InjectionKey<{
  open: (contextKey: string) => void;
  available: Ref<boolean>;
}> = Symbol("terminalPluginTools");
