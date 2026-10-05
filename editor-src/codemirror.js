// Bundle entry for src/vendor/codemirror.js: only what src/editor.js uses. Rebuild with `pnpm vendor`.
export { Annotation, EditorSelection, EditorState, Transaction } from "@codemirror/state";
export { Decoration, EditorView, keymap, placeholder, ViewPlugin, WidgetType } from "@codemirror/view";
export { defaultKeymap, history, historyKeymap, indentLess, indentMore, redo, undo } from "@codemirror/commands";
export { Language, LanguageSupport, syntaxTree } from "@codemirror/language";
export { deleteMarkupBackward, insertNewlineContinueMarkup, markdownLanguage } from "@codemirror/lang-markdown";
export { acceptCompletion, autocompletion, startCompletion } from "@codemirror/autocomplete";
