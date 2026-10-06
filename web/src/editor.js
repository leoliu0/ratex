// CodeMirror 6 with the LaTeX (stex) mode, one EditorState per open file.
import { EditorState, StateEffect, StateField } from '@codemirror/state';
import {
  EditorView, keymap, lineNumbers, highlightActiveLine, highlightActiveLineGutter,
  drawSelection, Decoration,
} from '@codemirror/view';
import { defaultKeymap, history, historyKeymap, indentWithTab } from '@codemirror/commands';
import { StreamLanguage, syntaxHighlighting, defaultHighlightStyle, bracketMatching } from '@codemirror/language';
import { searchKeymap, highlightSelectionMatches } from '@codemirror/search';
import { closeBrackets, closeBracketsKeymap } from '@codemirror/autocomplete';
import { stex } from '@codemirror/legacy-modes/mode/stex';

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
      syntaxHighlighting(defaultHighlightStyle, { fallback: true }),
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
