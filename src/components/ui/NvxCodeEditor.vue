<script setup lang="ts">
import { HighlightStyle, LanguageDescription, syntaxHighlighting } from "@codemirror/language";
import { languages } from "@codemirror/language-data";
import { Compartment, EditorState, type Extension } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { tags } from "@lezer/highlight";
import { basicSetup } from "codemirror";
import { onBeforeUnmount, onMounted, ref, watch } from "vue";

const props = withDefaults(defineProps<{
  modelValue: string;
  filename?: string;
  readonly?: boolean;
  followEnd?: boolean;
  label: string;
}>(), {
  filename: "text.txt",
  readonly: false,
  followEnd: false,
});

const emit = defineEmits<{
  "update:modelValue": [value: string];
}>();

const host = ref<HTMLElement | null>(null);
const editableCompartment = new Compartment();
const languageCompartment = new Compartment();
let editor: EditorView | null = null;
let applyingExternalValue = false;
let languageLoadRevision = 0;

const editorTheme = EditorView.theme({
  "&": {
    height: "100%",
    backgroundColor: "var(--nvx-color-bg-surface)",
    color: "var(--nvx-color-text-primary)",
    fontFamily: "var(--nvx-font-mono)",
    fontSize: "var(--nvx-font-size-sm)",
  },
  ".cm-scroller": {
    overflow: "auto",
    fontFamily: "inherit",
    lineHeight: "1.55",
  },
  ".cm-content": { caretColor: "var(--nvx-color-accent)" },
  ".cm-cursor, .cm-dropCursor": { borderLeftColor: "var(--nvx-color-accent)" },
  ".cm-gutters": {
    backgroundColor: "var(--nvx-color-bg-subtle)",
    color: "var(--nvx-color-text-tertiary)",
    borderRight: "var(--nvx-border-width) solid var(--nvx-color-border)",
  },
  ".cm-activeLine, .cm-activeLineGutter": { backgroundColor: "var(--nvx-color-bg-hover)" },
  "&.cm-focused": { outline: "none" },
  "&.cm-focused .cm-selectionBackground, ::selection": {
    backgroundColor: "var(--nvx-color-terminal-selection)",
  },
});

const editorHighlightStyle = HighlightStyle.define([
  { tag: [tags.comment, tags.meta], color: "var(--nvx-color-text-tertiary)" },
  { tag: [tags.keyword, tags.operatorKeyword, tags.controlKeyword], color: "var(--nvx-color-accent)" },
  { tag: [tags.string, tags.special(tags.string)], color: "var(--nvx-color-success)" },
  { tag: [tags.number, tags.bool, tags.null], color: "var(--nvx-color-warning)" },
  { tag: [tags.typeName, tags.className, tags.namespace], color: "var(--nvx-color-accent-hover)" },
  { tag: [tags.invalid], color: "var(--nvx-color-danger)", textDecoration: "underline" },
]);

function editableExtensions(readonly: boolean): Extension {
  return [
    EditorState.readOnly.of(readonly),
    EditorView.editable.of(!readonly),
    EditorView.contentAttributes.of({ tabindex: "0", "aria-label": props.label }),
  ];
}

async function loadLanguage(filename: string) {
  const revision = ++languageLoadRevision;
  const description = LanguageDescription.matchFilename(languages, filename);
  const support = description ? await description.load().catch(() => null) : null;
  if (revision !== languageLoadRevision || !editor) return;
  editor.dispatch({ effects: languageCompartment.reconfigure(support ?? []) });
}

function replaceDocument(value: string, followEnd: boolean) {
  if (!editor || editor.state.doc.toString() === value) return;
  const current = editor.state.doc.toString();
  const appendOnly = followEnd && value.startsWith(current);
  applyingExternalValue = true;
  editor.dispatch({
    changes: appendOnly
      ? { from: current.length, insert: value.slice(current.length) }
      : { from: 0, to: current.length, insert: value },
    selection: appendOnly ? { anchor: value.length } : undefined,
    scrollIntoView: followEnd,
  });
  applyingExternalValue = false;
}

onMounted(() => {
  if (!host.value) return;
  editor = new EditorView({
    parent: host.value,
    doc: props.modelValue,
    extensions: [
      basicSetup,
      EditorView.lineWrapping,
      editorTheme,
      syntaxHighlighting(editorHighlightStyle),
      editableCompartment.of(editableExtensions(props.readonly)),
      languageCompartment.of([]),
      EditorView.updateListener.of((update) => {
        if (update.docChanged && !applyingExternalValue) {
          emit("update:modelValue", update.state.doc.toString());
        }
      }),
    ],
  });
  void loadLanguage(props.filename);
});

watch(() => props.modelValue, (value) => replaceDocument(value, props.followEnd));
watch(() => props.readonly, (readonly) => {
  editor?.dispatch({ effects: editableCompartment.reconfigure(editableExtensions(readonly)) });
});
watch(() => props.filename, (filename) => void loadLanguage(filename));
watch(() => props.label, () => {
  editor?.dispatch({ effects: editableCompartment.reconfigure(editableExtensions(props.readonly)) });
});

onBeforeUnmount(() => {
  languageLoadRevision += 1;
  editor?.destroy();
  editor = null;
});

defineExpose({ focus: () => editor?.focus() });
</script>

<template>
  <div
    ref="host"
    class="nvx-code-editor"
  />
</template>

<style scoped>
.nvx-code-editor {
  width: 100%;
  height: 100%;
  min-width: 0;
  min-height: 0;
  overflow: hidden;
  background: var(--nvx-color-bg-surface);
}

.nvx-code-editor :deep(.cm-editor) {
  height: 100%;
}
</style>
