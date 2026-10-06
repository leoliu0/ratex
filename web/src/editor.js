// CodeMirror 6 with the LaTeX (stex) mode, one EditorState per open file.
import { EditorState, StateEffect, StateField } from '@codemirror/state';
import {
  EditorView, keymap, lineNumbers, highlightActiveLine, highlightActiveLineGutter,
  drawSelection, Decoration,
} from '@codemirror/view';
import { defaultKeymap, history, historyKeymap, indentWithTab } from '@codemirror/commands';
import { StreamLanguage, syntaxHighlighting, HighlightStyle, bracketMatching } from '@codemirror/language';
import { searchKeymap, highlightSelectionMatches } from '@codemirror/search';
import { closeBrackets, closeBracketsKeymap } from '@codemirror/autocomplete';
import { stex } from '@codemirror/legacy-modes/mode/stex';
import { tags } from '@lezer/highlight';

// Colours come from the page's CSS variables, so one theme serves light and dark.
const theme = EditorView.theme({
  '&': { color: 'var(--ink)', backgroundColor: 'var(--surface)', fontSize: '13.5px' },
  '&.cm-focused': { outline: 'none' },
  '.cm-scroller': { fontFamily: 'var(--mono)', lineHeight: '1.6' },
  '.cm-content': { padding: '10px 0 40vh', caretColor: 'var(--accent)' },
  '.cm-line': { padding: '0 16px 0 6px' },
  '.cm-gutters': { backgroundColor: 'var(--surface)', color: 'var(--ink-faint)', border: 'none' },
  '.cm-lineNumbers .cm-gutterElement': { padding: '0 8px 0 14px', minWidth: '46px', opacity: '0.75' },
  '.cm-activeLine': { backgroundColor: 'var(--hover)' },
  '.cm-activeLineGutter': { backgroundColor: 'transparent', color: 'var(--ink)' },
  '.cm-activeLineGutter.cm-gutterElement': { opacity: '1' },
  '.cm-cursor, .cm-dropCursor': { borderLeft: '2px solid var(--accent)' },
  '.cm-selectionBackground, &.cm-focused > .cm-scroller > .cm-selectionLayer .cm-selectionBackground':
    { backgroundColor: 'var(--selection)' },
  '.cm-selectionMatch': { backgroundColor: 'var(--warn-tint)' },
  '&.cm-focused .cm-matchingBracket': { backgroundColor: 'var(--accent-tint)', outline: '1px solid var(--accent-line)' },
  '&.cm-focused .cm-nonmatchingBracket': { backgroundColor: 'var(--danger-tint)' },
  '.cm-searchMatch': { backgroundColor: 'var(--warn-tint)', outline: '1px solid var(--warn)' },
  '.cm-searchMatch.cm-searchMatch-selected': { backgroundColor: 'var(--flash)' },
  '.cm-flash': { backgroundColor: 'var(--flash)' },
  '.cm-panels': { backgroundColor: 'var(--bg)', color: 'var(--ink)' },
  '.cm-panels.cm-panels-top': { borderBottom: '1px solid var(--line)' },
  '.cm-panels.cm-panels-bottom': { borderTop: '1px solid var(--line)' },
  '.cm-panel.cm-search': { padding: '6px 10px', fontFamily: 'var(--sans)' },
  '.cm-panel.cm-search input, .cm-panel.cm-search button, .cm-panel.cm-search label': { fontSize: '12px' },
  '.cm-textfield': {
    height: '26px', padding: '0 8px', border: '1px solid var(--line-strong)', borderRadius: '5px',
    backgroundColor: 'var(--surface)', color: 'var(--ink)',
  },
  '.cm-button': {
    height: '26px', padding: '0 9px', backgroundImage: 'none', backgroundColor: 'var(--surface)',
    border: '1px solid var(--line-strong)', borderRadius: '5px', color: 'var(--ink-soft)',
  },
  '.cm-tooltip': { backgroundColor: 'var(--surface)', border: '1px solid var(--line-strong)', borderRadius: '6px' },
});

const highlight = HighlightStyle.define([
  { tag: tags.tagName, color: 'var(--syn-command)' },
  { tag: tags.keyword, color: 'var(--syn-keyword)' },
  { tag: tags.atom, color: 'var(--syn-atom)' },
  { tag: [tags.bracket, tags.punctuation], color: 'var(--syn-bracket)' },
  { tag: tags.comment, color: 'var(--syn-comment)', fontStyle: 'italic' },
  { tag: tags.number, color: 'var(--syn-number)' },
  { tag: [tags.string, tags.special(tags.variableName)], color: 'var(--syn-string)' },
  { tag: tags.standard(tags.variableName), color: 'var(--syn-keyword)' },
  { tag: tags.invalid, color: 'var(--syn-error)' },
]);

const setFlash = StateEffect.define();
const flashField = StateField.define({
  create: () => Decoration.none,
  update(value, tr) {
    value = value.map(tr.changes);
    for (const effect of tr.effects) {
      if (effect.is(setFlash)) {
        value = effect.value === null
          ? Decoration.none
          : Decoration.set([Decoration.line({ class: 'cm-flash' }).range(effect.value)]);
      }
    }
    return value;
  },
  provide: (field) => EditorView.decorations.from(field),
});

export class Editor {
  constructor(parent, { onChange, onSyncRequest }) {
    this.onChange = onChange;
    this.states = new Map();
    this.path = null;
    this.extensions = [
      lineNumbers(),
      highlightActiveLineGutter(),
      history(),
      drawSelection(),
      bracketMatching(),
      closeBrackets(),
      highlightActiveLine(),
      highlightSelectionMatches(),
      theme,
      syntaxHighlighting(highlight),
      StreamLanguage.define(stex),
      EditorView.lineWrapping,
      flashField,
      keymap.of([
        { key: 'Mod-Enter', run: () => { onSyncRequest(); return true; } },
        ...closeBracketsKeymap, ...defaultKeymap, ...searchKeymap, ...historyKeymap, indentWithTab,
      ]),
      EditorView.updateListener.of((update) => {
        if (update.docChanged && this.path !== null) {
          this.onChange(this.path, update.state.doc.toString());
        }
      }),
    ];
    this.view = new EditorView({ parent, state: EditorState.create({ doc: '', extensions: this.extensions }) });
    this.view.contentDOM.setAttribute('data-testid', 'editor');
  }

  /** Show `path`, keeping each file's undo history and selection. */
  open(path, text) {
    if (this.path !== null) this.states.set(this.path, this.view.state);
    this.path = path;
    let state = this.states.get(path);
    if (!state || state.doc.toString() !== text) {
      state = EditorState.create({ doc: text, extensions: this.extensions });
    }
    this.view.setState(state);
  }

  close() {
    if (this.path !== null) this.states.set(this.path, this.view.state);
    this.path = null;
    this.view.setState(EditorState.create({ doc: '', extensions: this.extensions }));
  }

  forget(path) {
    this.states.delete(path);
    if (this.path === path) this.close();
  }

  rename(from, to) {
    const state = this.path === from ? this.view.state : this.states.get(from);
    this.states.delete(from);
    if (state) this.states.set(to, state);
    if (this.path === from) this.path = to;
  }

  /** 1-based line of the main cursor. */
  cursorLine() {
    const { state } = this.view;
    return state.doc.lineAt(state.selection.main.head).number;
  }

  goToLine(line, column = 1) {
    const { doc } = this.view.state;
    const target = doc.line(Math.min(Math.max(1, line), doc.lines));
    const pos = Math.min(target.from + Math.max(0, column - 1), target.to);
    this.view.dispatch({
      selection: { anchor: pos },
      effects: [EditorView.scrollIntoView(pos, { y: 'center' }), setFlash.of(target.from)],
    });
    this.view.focus();
    clearTimeout(this.flashTimer);
    this.flashTimer = setTimeout(() => this.view.dispatch({ effects: setFlash.of(null) }), 1500);
  }
}
