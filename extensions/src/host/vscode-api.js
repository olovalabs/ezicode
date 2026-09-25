'use strict';
// The `vscode` module ezicode injects into extensions.
//
// This is the same trick Eclipse Theia uses: extensions never link against a
// real `vscode` package (VS Code does not ship one), they just `require`
// the name and the host resolves it. So an editor only has to provide an
// object with the same shape — every member below is backed by an RPC call
// into the Rust side of ezicode.

const path = require('node:path');
const fsp = require('node:fs/promises');

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

class Disposable {
  constructor(callOnDispose) {
    this._callOnDispose = callOnDispose;
  }
  dispose() {
    if (typeof this._callOnDispose === 'function') {
      this._callOnDispose();
      this._callOnDispose = null;
    }
  }
  static from(...disposables) {
    return new Disposable(() => disposables.forEach((d) => d?.dispose?.()));
  }
}

class EventEmitter {
  constructor() {
    this._listeners = new Set();
    this.event = (listener, thisArg) => {
      const bound = thisArg ? listener.bind(thisArg) : listener;
      this._listeners.add(bound);
      return new Disposable(() => this._listeners.delete(bound));
    };
  }
  fire(value) {
    for (const listener of [...this._listeners]) {
      try {
        listener(value);
      } catch (err) {
        process.stderr.write(`[ezicode] event listener threw: ${err?.stack || err}\n`);
      }
    }
  }
  dispose() {
    this._listeners.clear();
  }
}

class Position {
  constructor(line, character) {
    this.line = line;
    this.character = character;
  }
  isEqual(other) {
    return other && this.line === other.line && this.character === other.character;
  }
  isBefore(other) {
    return this.line < other.line || (this.line === other.line && this.character < other.character);
  }
  translate(lineDelta = 0, characterDelta = 0) {
    return new Position(this.line + lineDelta, this.character + characterDelta);
  }
  with(line = this.line, character = this.character) {
    return new Position(line, character);
  }
}

class Range {
  constructor(startLine, startCharacter, endLine, endCharacter) {
    if (startLine instanceof Position) {
      this.start = startLine;
      this.end = startCharacter;
    } else {
      this.start = new Position(startLine, startCharacter);
      this.end = new Position(endLine, endCharacter);
    }
  }
  get isEmpty() {
    return this.start.isEqual(this.end);
  }
  get isSingleLine() {
    return this.start.line === this.end.line;
  }
  contains(position) {
    return !position.isBefore(this.start) && position.isBefore(this.end);
  }
}

class Selection extends Range {
  constructor(anchorLine, anchorCharacter, activeLine, activeCharacter) {
    super(anchorLine, anchorCharacter, activeLine, activeCharacter);
    this.anchor = this.start;
    this.active = this.end;
  }
}

class Location {
  constructor(uri, rangeOrPosition) {
    this.uri = uri;
    this.range = rangeOrPosition instanceof Position
      ? new Range(rangeOrPosition, rangeOrPosition)
      : rangeOrPosition;
  }
}

class Uri {
  constructor(scheme, authority, fsPath, query = '', fragment = '') {
    this.scheme = scheme;
    this.authority = authority;
    this.path = fsPath;
    this.query = query;
    this.fragment = fragment;
  }
  get fsPath() {
    if (process.platform === 'win32' && /^\/[a-zA-Z]:/.test(this.path)) return this.path.slice(1).replace(/\//g, '\\');
    return this.path;
  }
  toString() {
    const q = this.query ? `?${this.query}` : '';
    const f = this.fragment ? `#${this.fragment}` : '';
    return `${this.scheme}://${this.authority}${this.path}${q}${f}`;
  }
  toJSON() {
    return { scheme: this.scheme, authority: this.authority, path: this.path, query: this.query, fragment: this.fragment };
  }
  with(change = {}) {
    return new Uri(
      change.scheme ?? this.scheme,
      change.authority ?? this.authority,
      change.path ?? this.path,
      change.query ?? this.query,
      change.fragment ?? this.fragment
    );
  }
  static file(p) {
    let normalized = String(p).replace(/\\/g, '/');
    if (!normalized.startsWith('/')) normalized = '/' + normalized;
    return new Uri('file', '', normalized);
  }
  static parse(value) {
    const match = /^([a-zA-Z][a-zA-Z0-9+.-]*):\/\/([^/?#]*)([^?#]*)(?:\?([^#]*))?(?:#(.*))?$/.exec(String(value));
    if (!match) return Uri.file(value);
    return new Uri(match[1], match[2] || '', match[3] || '/', match[4] || '', match[5] || '');
  }
  static joinPath(base, ...segments) {
    return base.with({ path: path.posix.join(base.path, ...segments) });
  }
}

class MarkdownString {
  constructor(value = '') {
    this.value = value;
    this.isTrusted = false;
    this.supportThemeIcons = false;
  }
  appendText(text) {
    this.value += text.replace(/[\\`*_{}[\]()#+\-.!]/g, '\\$&');
    return this;
  }
  appendMarkdown(text) {
    this.value += text;
    return this;
  }
  appendCodeblock(code, language = '') {
    this.value += `\n\`\`\`${language}\n${code}\n\`\`\`\n`;
    return this;
  }
}

class Hover {
  constructor(contents, range) {
    this.contents = Array.isArray(contents) ? contents : [contents];
    this.range = range;
  }
}

class CompletionItem {
  constructor(label, kind) {
    this.label = label;
    this.kind = kind;
  }
}

class TextEdit {
  constructor(range, newText) {
    this.range = range;
    this.newText = newText;
  }
  static insert(position, newText) {
    return new TextEdit(new Range(position, position), newText);
  }
  static replace(range, newText) {
    return new TextEdit(range, newText);
  }
  static delete(range) {
    return new TextEdit(range, '');
  }
}

class WorkspaceEdit {
  constructor() {
    this._edits = new Map();
  }
  insert(uri, position, newText) {
    this.replace(uri, new Range(position, position), newText);
  }
  replace(uri, range, newText) {
    const key = uri.toString();
    if (!this._edits.has(key)) this._edits.set(key, { uri, edits: [] });
    this._edits.get(key).edits.push(new TextEdit(range, newText));
  }
  delete(uri, range) {
    this.replace(uri, range, '');
  }
  entries() {
    return [...this._edits.values()].map((e) => [e.uri, e.edits]);
  }
  get size() {
    return this._edits.size;
  }
}

class Diagnostic {
  constructor(range, message, severity = 0) {
    this.range = range;
    this.message = message;
    this.severity = severity;
  }
}

class CancellationTokenSource {
  constructor() {
    this._emitter = new EventEmitter();
    this.token = { isCancellationRequested: false, onCancellationRequested: this._emitter.event };
  }
  cancel() {
    this.token.isCancellationRequested = true;
    this._emitter.fire();
  }
  dispose() {
    this._emitter.dispose();
  }
}

const CompletionItemKind = {
  Text: 0, Method: 1, Function: 2, Constructor: 3, Field: 4, Variable: 5, Class: 6,
  Interface: 7, Module: 8, Property: 9, Unit: 10, Value: 11, Enum: 12, Keyword: 13,
  Snippet: 14, Color: 15, File: 16, Reference: 17, Folder: 18, EnumMember: 19,
  Constant: 20, Struct: 21, Event: 22, Operator: 23, TypeParameter: 24,
};
const StatusBarAlignment = { Left: 1, Right: 2 };
const DiagnosticSeverity = { Error: 0, Warning: 1, Information: 2, Hint: 3 };
const ConfigurationTarget = { Global: 1, Workspace: 2, WorkspaceFolder: 3 };
const ExtensionMode = { Production: 1, Development: 2, Test: 3 };
const ViewColumn = { Active: -1, Beside: -2, One: 1, Two: 2, Three: 3 };
const EndOfLine = { LF: 1, CRLF: 2 };
const FileType = { Unknown: 0, File: 1, Directory: 2, SymbolicLink: 64 };
const ProgressLocation = { SourceControl: 1, Window: 10, Notification: 15 };

class ThemeColor {
  constructor(id) {
    this.id = id;
  }
}
class ThemeIcon {
  constructor(id, color) {
    this.id = id;
    this.color = color;
  }
}

// ---------------------------------------------------------------------------
// Namespaces
// ---------------------------------------------------------------------------

/**
 * Build the `vscode` module object handed to one extension.
 *
 * @param {import('./host').ExtensionHost} host
 * @param {object} extension the host's record for this extension
 */
function createVscodeApi(host, extension) {
  const subscriptionsOf = (disposable) => {
    extension.disposables.push(disposable);
    return disposable;
  };

  const commands = {
    registerCommand(command, callback, thisArg) {
      host.registerCommand(extension, command, thisArg ? callback.bind(thisArg) : callback);
      return subscriptionsOf(new Disposable(() => host.unregisterCommand(command)));
    },
    registerTextEditorCommand(command, callback, thisArg) {
      return commands.registerCommand(
        command,
        (...args) => callback(host.activeTextEditor(), { insert: () => {} }, ...args),
        thisArg
      );
    },
    executeCommand(command, ...args) {
      return host.executeCommand(command, args);
    },
    getCommands() {
      return Promise.resolve(host.commandIds());
    },
  };

  const window = {
    get activeTextEditor() {
      return host.activeTextEditor();
    },
    get visibleTextEditors() {
      const editor = host.activeTextEditor();
      return editor ? [editor] : [];
    },
    showInformationMessage: (message, ...items) => host.showMessage('info', message, items),
    showWarningMessage: (message, ...items) => host.showMessage('warning', message, items),
    showErrorMessage: (message, ...items) => host.showMessage('error', message, items),
    showQuickPick: (items, options) => host.request('window/showQuickPick', { items, options }),
    showInputBox: (options) => host.request('window/showInputBox', { options }),
    setStatusBarMessage(text, hideAfterTimeout) {
      host.notify('window/setStatusBarMessage', { text, hideAfterTimeout: hideAfterTimeout ?? null });
      return new Disposable(() => host.notify('window/setStatusBarMessage', { text: null }));
    },
    createStatusBarItem(alignment = StatusBarAlignment.Left, priority = 0) {
      const id = host.nextHandle('statusbar');
      const item = {
        id,
        alignment,
        priority,
        text: '',
        tooltip: '',
        command: undefined,
        show: () => host.notify('window/statusBarItem', { id, action: 'show', text: item.text, tooltip: item.tooltip, command: item.command, alignment, priority }),
        hide: () => host.notify('window/statusBarItem', { id, action: 'hide' }),
        dispose: () => host.notify('window/statusBarItem', { id, action: 'dispose' }),
      };
      return subscriptionsOf(item);
    },
    createOutputChannel(name) {
      const channel = {
        name,
        append: (value) => host.notify('window/outputChannel', { name, value }),
        appendLine: (value) => host.notify('window/outputChannel', { name, value: value + '\n' }),
        clear: () => host.notify('window/outputChannel', { name, clear: true }),
        show: () => host.notify('window/outputChannel', { name, show: true }),
        hide: () => {},
        dispose: () => {},
      };
      return subscriptionsOf(channel);
    },
    withProgress(_options, task) {
      return Promise.resolve(task({ report: () => {} }, new CancellationTokenSource().token));
    },
    onDidChangeActiveTextEditor: (listener) => subscriptionsOf(host.events.activeEditor.event(listener)),
    showTextDocument: (documentOrUri, _options) => host.showTextDocument(documentOrUri),
  };

  const workspace = {
    get workspaceFolders() {
      return host.workspaceFolders();
    },
    get rootPath() {
      return host.workspaceFolders()[0]?.uri.fsPath;
    },
    get name() {
      const folder = host.workspaceFolders()[0];
      return folder ? path.basename(folder.uri.fsPath) : undefined;
    },
    get textDocuments() {
      return host.documents();
    },
    getConfiguration: (section, _scope) => host.getConfiguration(section),
    openTextDocument: (uriOrPath) => host.openTextDocument(uriOrPath),
    asRelativePath(pathOrUri, includeWorkspaceFolder = false) {
      const target = typeof pathOrUri === 'string' ? pathOrUri : pathOrUri.fsPath;
      const root = host.workspaceFolders()[0]?.uri.fsPath;
      if (!root) return target;
      const rel = path.relative(root, target);
      return includeWorkspaceFolder ? path.join(path.basename(root), rel) : rel;
    },
    onDidOpenTextDocument: (listener) => subscriptionsOf(host.events.openDocument.event(listener)),
    onDidCloseTextDocument: (listener) => subscriptionsOf(host.events.closeDocument.event(listener)),
    onDidChangeTextDocument: (listener) => subscriptionsOf(host.events.changeDocument.event(listener)),
    onDidSaveTextDocument: (listener) => subscriptionsOf(host.events.saveDocument.event(listener)),
    onDidChangeConfiguration: (listener) => subscriptionsOf(host.events.changeConfiguration.event(listener)),
    applyEdit: (edit) =>
      host.request('workspace/applyEdit', {
        edits: edit.entries().map(([uri, edits]) => ({
          uri: uri.toString(),
          edits: edits.map((e) => ({ range: e.range, newText: e.newText })),
        })),
      }),
    fs: {
      readFile: async (uri) => new Uint8Array(await fsp.readFile(uri.fsPath)),
      writeFile: (uri, content) => fsp.writeFile(uri.fsPath, Buffer.from(content)),
      delete: (uri, options = {}) => fsp.rm(uri.fsPath, { recursive: Boolean(options.recursive), force: true }),
      createDirectory: (uri) => fsp.mkdir(uri.fsPath, { recursive: true }),
      async readDirectory(uri) {
        const entries = await fsp.readdir(uri.fsPath, { withFileTypes: true });
        return entries.map((e) => [e.name, e.isDirectory() ? FileType.Directory : FileType.File]);
      },
      async stat(uri) {
        const s = await fsp.stat(uri.fsPath);
        return { type: s.isDirectory() ? FileType.Directory : FileType.File, ctime: s.ctimeMs, mtime: s.mtimeMs, size: s.size };
      },
    },
  };

  const languages = {
    registerHoverProvider(selector, provider) {
      const handle = host.registerProvider('hover', extension, selector, provider);
      return subscriptionsOf(new Disposable(() => host.unregisterProvider('hover', handle)));
    },
    registerCompletionItemProvider(selector, provider, ...triggerCharacters) {
      const handle = host.registerProvider('completion', extension, selector, provider, { triggerCharacters });
      return subscriptionsOf(new Disposable(() => host.unregisterProvider('completion', handle)));
    },
    registerDocumentFormattingEditProvider(selector, provider) {
      const handle = host.registerProvider('formatting', extension, selector, provider);
      return subscriptionsOf(new Disposable(() => host.unregisterProvider('formatting', handle)));
    },
    registerCodeActionsProvider(selector, provider) {
      const handle = host.registerProvider('codeAction', extension, selector, provider);
      return subscriptionsOf(new Disposable(() => host.unregisterProvider('codeAction', handle)));
    },
    createDiagnosticCollection(name = 'ezicode') {
      const collection = {
        name,
        set: (uri, diagnostics) =>
          host.notify('languages/diagnostics', {
            owner: name,
            uri: uri?.toString?.() ?? String(uri),
            diagnostics: diagnostics || [],
          }),
        delete: (uri) => host.notify('languages/diagnostics', { owner: name, uri: uri.toString(), diagnostics: [] }),
        clear: () => host.notify('languages/diagnostics', { owner: name, clear: true }),
        dispose: () => host.notify('languages/diagnostics', { owner: name, clear: true }),
      };
      return subscriptionsOf(collection);
    },
    getLanguages: () => Promise.resolve(host.knownLanguages()),
  };

  const env = {
    appName: 'ezicode',
    appHost: 'desktop',
    language: 'en',
    machineId: 'ezicode',
    sessionId: host.sessionId,
    uriScheme: 'ezicode',
    clipboard: {
      readText: () => host.request('env/clipboardRead', {}),
      writeText: (text) => host.request('env/clipboardWrite', { text }),
    },
    openExternal: (uri) => host.request('env/openExternal', { uri: uri.toString() }),
  };

  const extensionsNs = {
    all: host.extensionDescriptors(),
    getExtension: (id) => host.extensionDescriptors().find((e) => e.id.toLowerCase() === String(id).toLowerCase()),
    onDidChange: (listener) => subscriptionsOf(host.events.extensionsChanged.event(listener)),
  };

  return {
    version: host.apiVersion,
    // namespaces
    commands,
    window,
    workspace,
    languages,
    env,
    extensions: extensionsNs,
    // types
    Disposable,
    EventEmitter,
    Position,
    Range,
    Selection,
    Location,
    Uri,
    MarkdownString,
    Hover,
    CompletionItem,
    CompletionItemKind,
    TextEdit,
    WorkspaceEdit,
    Diagnostic,
    DiagnosticSeverity,
    CancellationTokenSource,
    StatusBarAlignment,
    ConfigurationTarget,
    ExtensionMode,
    ViewColumn,
    EndOfLine,
    FileType,
    ProgressLocation,
    ThemeColor,
    ThemeIcon,
  };
}

module.exports = {
  createVscodeApi,
  Disposable,
  EventEmitter,
  Position,
  Range,
  Selection,
  Uri,
  TextEdit,
  MarkdownString,
};
