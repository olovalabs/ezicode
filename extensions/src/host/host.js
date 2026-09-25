'use strict';
// ezicode extension host.
//
// A separate Node process that loads VS Code-compatible extensions and talks
// JSON-RPC to the editor over stdio. Extensions cannot touch the editor
// directly: everything they do arrives in Rust as a message, which is what
// keeps a misbehaving extension from freezing the UI thread.
//
//   editor (Rust, GPUI)  <──Content-Length JSON-RPC──>  host (Node)  ──> extensions
//
// Run standalone:  node src/host/host.js --extensions-dir ~/.local/share/ezicode/extensions

const fs = require('node:fs');
const path = require('node:path');
const Module = require('node:module');

const { Connection } = require('./rpc');
const { createVscodeApi, Disposable, EventEmitter, Position, Range, Selection, Uri } = require('./vscode-api');
const jsonc = require('../jsonc');
const { readManifest, isSupportedActivation } = require('../manifest');

/** The VS Code API version this shim claims to implement. */
const API_VERSION = '1.85.0';

// ---------------------------------------------------------------------------
// Documents
// ---------------------------------------------------------------------------

class TextDocument {
  constructor({ uri, languageId, text, version = 1 }) {
    this.uri = typeof uri === 'string' ? Uri.parse(uri) : uri;
    this.languageId = languageId || 'plaintext';
    this.version = version;
    this._text = text ?? '';
    this.isUntitled = this.uri.scheme === 'untitled';
    this.isDirty = false;
    this.isClosed = false;
  }
  get fileName() {
    return this.uri.fsPath;
  }
  get lineCount() {
    return this._text.split('\n').length;
  }
  get eol() {
    return this._text.includes('\r\n') ? 2 : 1;
  }
  getText(range) {
    if (!range) return this._text;
    return this._text.slice(this.offsetAt(range.start), this.offsetAt(range.end));
  }
  lineAt(lineOrPosition) {
    const line = typeof lineOrPosition === 'number' ? lineOrPosition : lineOrPosition.line;
    const lines = this._text.split('\n');
    const text = lines[line] ?? '';
    return {
      lineNumber: line,
      text,
      range: new Range(line, 0, line, text.length),
      rangeIncludingLineBreak: new Range(line, 0, line + 1, 0),
      firstNonWhitespaceCharacterIndex: text.length - text.trimStart().length,
      isEmptyOrWhitespace: text.trim().length === 0,
    };
  }
  offsetAt(position) {
    const lines = this._text.split('\n');
    let offset = 0;
    for (let i = 0; i < position.line && i < lines.length; i++) offset += lines[i].length + 1;
    return offset + position.character;
  }
  positionAt(offset) {
    const before = this._text.slice(0, offset).split('\n');
    return new Position(before.length - 1, before[before.length - 1].length);
  }
  getWordRangeAtPosition(position, regex = /[A-Za-z_$][\w$]*/g) {
    const line = this.lineAt(position).text;
    const re = new RegExp(regex.source, regex.flags.includes('g') ? regex.flags : regex.flags + 'g');
    let match;
    while ((match = re.exec(line))) {
      const start = match.index;
      const end = start + match[0].length;
      if (position.character >= start && position.character <= end) {
        return new Range(position.line, start, position.line, end);
      }
    }
    return undefined;
  }
  _update(text, version) {
    this._text = text;
    this.version = version ?? this.version + 1;
  }
}

function matchesSelector(selector, document) {
  if (!selector) return true;
  const list = Array.isArray(selector) ? selector : [selector];
  return list.some((filter) => {
    if (typeof filter === 'string') return filter === '*' || filter === document.languageId;
    if (filter.language && filter.language !== '*' && filter.language !== document.languageId) return false;
    if (filter.scheme && filter.scheme !== '*' && filter.scheme !== document.uri.scheme) return false;
    if (filter.pattern) {
      const rx = new RegExp(
        '^' + String(filter.pattern).replace(/[.+^${}()|[\]\\]/g, '\\$&').replace(/\*\*/g, '.*').replace(/(?<!\.)\*/g, '[^/]*') + '$'
      );
      if (!rx.test(document.uri.fsPath.replace(/\\/g, '/'))) return false;
    }
    return true;
  });
}

// ---------------------------------------------------------------------------
// Host
// ---------------------------------------------------------------------------

class ExtensionHost {
  /**
   * @param {{connection: Connection, extensionsDir: string, storageDir?: string}} options
   */
  constructor({ connection, extensionsDir, storageDir }) {
    this.connection = connection;
    this.extensionsDir = extensionsDir;
    this.storageDir = storageDir || path.join(extensionsDir, '.state');
    this.apiVersion = API_VERSION;
    this.sessionId = `ezicode-${process.pid}`;

    /** @type {Map<string, object>} */
    this.extensions = new Map();
    /** @type {Map<string, {extension: object, callback: Function}>} */
    this.commands = new Map();
    /** @type {Map<string, {kind: string, selector: any, provider: any, extension: object, meta: object}>} */
    this.providers = new Map();
    /** @type {Map<string, TextDocument>} */
    this.docs = new Map();

    this.activeUri = null;
    this.activeSelection = new Selection(0, 0, 0, 0);
    this.settings = {};
    this.folders = [];
    this.handleCounter = 0;

    this.events = {
      activeEditor: new EventEmitter(),
      openDocument: new EventEmitter(),
      closeDocument: new EventEmitter(),
      changeDocument: new EventEmitter(),
      saveDocument: new EventEmitter(),
      changeConfiguration: new EventEmitter(),
      extensionsChanged: new EventEmitter(),
    };

    this._installRequireHook();
    this._registerRpc();
  }

  // -- plumbing -------------------------------------------------------------

  nextHandle(prefix = 'h') {
    this.handleCounter += 1;
    return `${prefix}-${this.handleCounter}`;
  }

  notify(method, params) {
    this.connection.sendNotification(method, params);
  }

  request(method, params) {
    return this.connection.sendRequest(method, params);
  }

  log(level, message) {
    this.notify('log', { level, message });
  }

  /**
   * Make `require('vscode')` resolve to the API object of whichever extension
   * is doing the requiring. Same mechanism Theia uses for VS Code plugins.
   */
  _installRequireHook() {
    const host = this;
    const originalLoad = Module._load;
    Module._load = function (request, parent, isMain) {
      if (request === 'vscode') {
        const extension = host._extensionForFile(parent?.filename);
        return extension ? extension.api : createVscodeApi(host, host._anonymousExtension());
      }
      return originalLoad.apply(this, arguments);
    };
  }

  _extensionForFile(file) {
    if (!file) return null;
    const resolved = path.resolve(file);
    for (const extension of this.extensions.values()) {
      if (resolved.startsWith(extension.path + path.sep)) return extension;
    }
    return null;
  }

  _anonymousExtension() {
    if (!this._anon) {
      this._anon = { id: 'ezicode.anonymous', path: this.extensionsDir, disposables: [] };
    }
    return this._anon;
  }

  // -- discovery ------------------------------------------------------------

  /** Scan the extensions directory. Safe to call again after an install. */
  discover() {
    const found = [];
    if (!fs.existsSync(this.extensionsDir)) return found;

    for (const entry of fs.readdirSync(this.extensionsDir, { withFileTypes: true })) {
      if (!entry.isDirectory() || entry.name.startsWith('.')) continue;
      const dir = path.join(this.extensionsDir, entry.name);
      const manifestPath = path.join(dir, 'package.json');
      if (!fs.existsSync(manifestPath)) continue;

      let manifest;
      try {
        manifest = readManifest(jsonc.parse(fs.readFileSync(manifestPath, 'utf8'), manifestPath));
      } catch (err) {
        this.log('error', `skipping ${entry.name}: ${err.message}`);
        continue;
      }

      let enabled = true;
      try {
        const meta = JSON.parse(fs.readFileSync(path.join(dir, '.ezicode.json'), 'utf8'));
        enabled = meta.enabled !== false;
      } catch {
        /* no install metadata: side-loaded folder, treat as enabled */
      }

      if (this.extensions.has(manifest.id)) {
        found.push(this.extensions.get(manifest.id));
        continue;
      }

      const extension = {
        id: manifest.id,
        manifest,
        path: path.resolve(dir),
        enabled,
        active: false,
        exports: undefined,
        disposables: [],
        api: null,
        module: null,
      };
      extension.api = createVscodeApi(this, extension);
      this.extensions.set(extension.id, extension);
      found.push(extension);
    }
    this.events.extensionsChanged.fire();
    return found;
  }

  extensionDescriptors() {
    return [...this.extensions.values()].map((extension) => ({
      id: extension.id,
      extensionPath: extension.path,
      extensionUri: Uri.file(extension.path),
      packageJSON: extension.manifest,
      isActive: extension.active,
      exports: extension.exports,
      activate: () => this.activate(extension),
    }));
  }

  // -- activation -----------------------------------------------------------

  shouldActivate(extension, event) {
    if (!extension.enabled || extension.active) return false;
    const events = extension.manifest.activationEvents;
    if (!events.length) return event === '*';
    if (events.includes('*')) return true;
    if (!event) return false;
    if (events.includes(event)) return true;
    if (event === 'onStartupFinished') return events.includes('onStartupFinished');
    return false;
  }

  /** Activate every extension whose activation events match `event`. */
  async activateByEvent(event) {
    const activated = [];
    for (const extension of this.extensions.values()) {
      const events = extension.manifest.activationEvents;
      const matches =
        events.includes('*') ||
        events.includes(event) ||
        (event.startsWith('onLanguage:') && events.includes(event)) ||
        (event.startsWith('onCommand:') && events.includes(event));
      if (matches && extension.enabled && !extension.active) {
        await this.activate(extension);
        activated.push(extension.id);
      }
    }
    return activated;
  }

  async activate(extension) {
    if (typeof extension === 'string') extension = this.extensions.get(extension);
    if (!extension) throw new Error('unknown extension');
    if (extension.active) return extension.exports;
    if (!extension.enabled) throw new Error(`${extension.id} is disabled`);

    const main = extension.manifest.main;
    if (!main) {
      // Declarative extension (themes, snippets…): nothing to run.
      extension.active = true;
      return undefined;
    }

    const entry = path.resolve(extension.path, main);
    if (!fs.existsSync(entry)) throw new Error(`${extension.id}: main "${main}" not found`);

    const context = this._createContext(extension);
    const loaded = require(entry);
    extension.module = loaded;
    extension.active = true;
    if (typeof loaded.activate === 'function') {
      extension.exports = await loaded.activate(context);
    }
    this.log('info', `activated ${extension.id}`);
    this.notify('extension/activated', { id: extension.id, commands: this.commandIds() });
    return extension.exports;
  }

  async deactivate(extension) {
    if (typeof extension === 'string') extension = this.extensions.get(extension);
    if (!extension || !extension.active) return;
    try {
      if (typeof extension.module?.deactivate === 'function') await extension.module.deactivate();
    } finally {
      for (const disposable of extension.disposables.splice(0)) {
        try {
          disposable.dispose?.();
        } catch {
          /* ignore */
        }
      }
      for (const [id, entry] of [...this.commands]) {
        if (entry.extension === extension) this.commands.delete(id);
      }
      for (const [handle, entry] of [...this.providers]) {
        if (entry.extension === extension) this.providers.delete(handle);
      }
      extension.active = false;
    }
  }

  _memento(file) {
    const load = () => {
      try {
        return JSON.parse(fs.readFileSync(file, 'utf8'));
      } catch {
        return {};
      }
    };
    let state = load();
    return {
      keys: () => Object.keys(state),
      get: (key, fallback) => (key in state ? state[key] : fallback),
      update: (key, value) => {
        if (value === undefined) delete state[key];
        else state[key] = value;
        fs.mkdirSync(path.dirname(file), { recursive: true });
        fs.writeFileSync(file, JSON.stringify(state, null, 2));
        return Promise.resolve();
      },
      setKeysForSync: () => {},
    };
  }

  _createContext(extension) {
    const storage = path.join(this.storageDir, extension.id);
    return {
      subscriptions: extension.disposables,
      extensionPath: extension.path,
      extensionUri: Uri.file(extension.path),
      extensionMode: 1,
      environmentVariableCollection: { replace: () => {}, append: () => {}, prepend: () => {}, clear: () => {} },
      asAbsolutePath: (relative) => path.join(extension.path, relative),
      globalState: this._memento(path.join(storage, 'global.json')),
      workspaceState: this._memento(path.join(storage, 'workspace.json')),
      storageUri: Uri.file(storage),
      globalStorageUri: Uri.file(storage),
      logUri: Uri.file(path.join(storage, 'logs')),
      secrets: {
        get: () => Promise.resolve(undefined),
        store: () => Promise.resolve(),
        delete: () => Promise.resolve(),
        onDidChange: new EventEmitter().event,
      },
      extension: { id: extension.id, packageJSON: extension.manifest },
    };
  }

  // -- API surface used by the shim ----------------------------------------

  registerCommand(extension, command, callback) {
    if (this.commands.has(command)) throw new Error(`command already registered: ${command}`);
    this.commands.set(command, { extension, callback });
    this.notify('commands/registered', { command, extension: extension.id });
  }

  unregisterCommand(command) {
    this.commands.delete(command);
    this.notify('commands/unregistered', { command });
  }

  commandIds() {
    return [...this.commands.keys()];
  }

  /** Command metadata merged from every manifest, for the command palette. */
  contributedCommands() {
    const out = [];
    for (const extension of this.extensions.values()) {
      for (const command of extension.manifest.contributes?.commands || []) {
        out.push({
          command: command.command,
          title: command.title,
          category: command.category || extension.manifest.displayName,
          extension: extension.id,
          icon: typeof command.icon === 'string' ? command.icon : undefined,
        });
      }
    }
    return out;
  }

  async executeCommand(command, args = []) {
    if (!this.commands.has(command)) {
      // Lazy activation: `onCommand:x` extensions only load when x is invoked.
      await this.activateByEvent(`onCommand:${command}`);
    }
    const entry = this.commands.get(command);
    if (!entry) throw new Error(`command not found: ${command}`);
    return await entry.callback(...args);
  }

  registerProvider(kind, extension, selector, provider, meta = {}) {
    const handle = this.nextHandle(kind);
    this.providers.set(handle, { kind, extension, selector, provider, meta });
    this.notify('providers/changed', { kind, handle, action: 'register' });
    return handle;
  }

  unregisterProvider(kind, handle) {
    this.providers.delete(handle);
    this.notify('providers/changed', { kind, handle, action: 'unregister' });
  }

  showMessage(kind, message, items = []) {
    return this.request('window/showMessage', { kind, message, items });
  }

  getConfiguration(section) {
    const prefix = section ? `${section}.` : '';
    const read = (key) => {
      const full = prefix + key;
      if (full in this.settings) return this.settings[full];
      // Also support nested objects: {"editor": {"fontSize": 14}}
      return full.split('.').reduce((acc, part) => (acc && typeof acc === 'object' ? acc[part] : undefined), this.settings);
    };
    const host = this;
    return {
      get(key, fallback) {
        const value = read(key);
        return value === undefined ? fallback : value;
      },
      has: (key) => read(key) !== undefined,
      inspect: (key) => ({ key: prefix + key, globalValue: read(key) }),
      update(key, value) {
        host.settings[prefix + key] = value;
        return host.request('configuration/update', { key: prefix + key, value });
      },
    };
  }

  workspaceFolders() {
    return this.folders.map((folder, index) => ({
      uri: Uri.file(folder),
      name: path.basename(folder),
      index,
    }));
  }

  documents() {
    return [...this.docs.values()];
  }

  knownLanguages() {
    return [...new Set([...this.docs.values()].map((d) => d.languageId))];
  }

  openTextDocument(uriOrPath) {
    const uri = typeof uriOrPath === 'string' ? Uri.file(uriOrPath) : uriOrPath?.uri ? uriOrPath.uri : uriOrPath;
    const key = uri.toString();
    if (this.docs.has(key)) return Promise.resolve(this.docs.get(key));
    const text = fs.existsSync(uri.fsPath) ? fs.readFileSync(uri.fsPath, 'utf8') : '';
    const doc = new TextDocument({ uri, languageId: languageFor(uri.fsPath), text });
    this.docs.set(key, doc);
    this.events.openDocument.fire(doc);
    return Promise.resolve(doc);
  }

  showTextDocument(documentOrUri) {
    const uri = documentOrUri?.uri ?? documentOrUri;
    return this.request('window/showTextDocument', { uri: uri.toString() }).then(() => this.activeTextEditor());
  }

  activeTextEditor() {
    if (!this.activeUri) return undefined;
    const document = this.docs.get(this.activeUri);
    if (!document) return undefined;
    const host = this;
    return {
      document,
      selection: host.activeSelection,
      selections: [host.activeSelection],
      viewColumn: 1,
      async edit(callback) {
        const edits = [];
        callback({
          insert: (position, text) => edits.push({ range: new Range(position, position), newText: text }),
          replace: (range, text) => edits.push({ range, newText: text }),
          delete: (range) => edits.push({ range, newText: '' }),
        });
        await host.request('workspace/applyEdit', {
          edits: [{ uri: document.uri.toString(), edits }],
        });
        return true;
      },
      revealRange: (range) => host.notify('editor/revealRange', { uri: document.uri.toString(), range }),
    };
  }

  // -- RPC surface ----------------------------------------------------------

  _registerRpc() {
    const c = this.connection;

    c.onRequest('initialize', async (params = {}) => {
      this.folders = params.workspaceFolders || [];
      this.settings = params.settings || {};
      if (params.extensionsDir) this.extensionsDir = params.extensionsDir;
      this.discover();
      return {
        apiVersion: this.apiVersion,
        pid: process.pid,
        extensions: [...this.extensions.values()].map((e) => ({
          id: e.id,
          displayName: e.manifest.displayName,
          version: e.manifest.version,
          publisher: e.manifest.publisher,
          description: e.manifest.description,
          enabled: e.enabled,
          lazy: Boolean(e.manifest.main),
          activationEvents: e.manifest.activationEvents,
          unsupportedActivation: e.manifest.activationEvents.filter((ev) => !isSupportedActivation(ev)),
        })),
        commands: this.contributedCommands(),
      };
    });

    c.onRequest('extensions/list', () => ({
      extensions: [...this.extensions.values()].map((e) => ({
        id: e.id,
        displayName: e.manifest.displayName,
        version: e.manifest.version,
        active: e.active,
        enabled: e.enabled,
      })),
    }));

    c.onRequest('activate', async (params = {}) => {
      if (params.id) {
        await this.activate(params.id);
        return { activated: [params.id] };
      }
      return { activated: await this.activateByEvent(params.event || 'onStartupFinished') };
    });

    c.onRequest('deactivate', async (params = {}) => {
      await this.deactivate(params.id);
      return { ok: true };
    });

    c.onRequest('commands/list', () => ({
      registered: this.commandIds(),
      contributed: this.contributedCommands(),
    }));

    c.onRequest('commands/execute', async (params = {}) => ({
      result: (await this.executeCommand(params.command, params.args || [])) ?? null,
    }));

    c.onNotification('document/didOpen', (params = {}) => {
      const doc = new TextDocument(params);
      this.docs.set(doc.uri.toString(), doc);
      this.events.openDocument.fire(doc);
      void this.activateByEvent(`onLanguage:${doc.languageId}`);
    });

    c.onNotification('document/didChange', (params = {}) => {
      const doc = this.docs.get(Uri.parse(params.uri).toString());
      if (!doc) return;
      doc._update(params.text, params.version);
      this.events.changeDocument.fire({ document: doc, contentChanges: [], reason: undefined });
    });

    c.onNotification('document/didSave', (params = {}) => {
      const doc = this.docs.get(Uri.parse(params.uri).toString());
      if (doc) this.events.saveDocument.fire(doc);
    });

    c.onNotification('document/didClose', (params = {}) => {
      const key = Uri.parse(params.uri).toString();
      const doc = this.docs.get(key);
      if (!doc) return;
      doc.isClosed = true;
      this.docs.delete(key);
      this.events.closeDocument.fire(doc);
    });

    c.onNotification('editor/didChangeActive', (params = {}) => {
      this.activeUri = params.uri ? Uri.parse(params.uri).toString() : null;
      if (params.selection) {
        const s = params.selection;
        this.activeSelection = new Selection(s.startLine ?? 0, s.startCharacter ?? 0, s.endLine ?? 0, s.endCharacter ?? 0);
      }
      this.events.activeEditor.fire(this.activeTextEditor());
    });

    c.onNotification('configuration/didChange', (params = {}) => {
      this.settings = params.settings || {};
      this.events.changeConfiguration.fire({ affectsConfiguration: () => true });
    });

    c.onRequest('provider/hover', async (params = {}) => {
      const doc = this.docs.get(Uri.parse(params.uri).toString());
      if (!doc) return { contents: [] };
      const position = new Position(params.position?.line ?? 0, params.position?.character ?? 0);
      for (const entry of this.providers.values()) {
        if (entry.kind !== 'hover' || !matchesSelector(entry.selector, doc)) continue;
        const hover = await entry.provider.provideHover(doc, position, noCancel());
        if (hover) {
          return {
            contents: (Array.isArray(hover.contents) ? hover.contents : [hover.contents]).map(stringifyMarkdown),
            range: hover.range ?? null,
            extension: entry.extension.id,
          };
        }
      }
      return { contents: [] };
    });

    c.onRequest('provider/completion', async (params = {}) => {
      const doc = this.docs.get(Uri.parse(params.uri).toString());
      if (!doc) return { items: [] };
      const position = new Position(params.position?.line ?? 0, params.position?.character ?? 0);
      const items = [];
      for (const entry of this.providers.values()) {
        if (entry.kind !== 'completion' || !matchesSelector(entry.selector, doc)) continue;
        const result = await entry.provider.provideCompletionItems(doc, position, noCancel(), {
          triggerKind: params.triggerCharacter ? 1 : 0,
          triggerCharacter: params.triggerCharacter,
        });
        const list = Array.isArray(result) ? result : result?.items || [];
        for (const item of list) {
          items.push({
            label: typeof item.label === 'string' ? item.label : item.label?.label,
            kind: item.kind ?? 0,
            detail: item.detail ?? null,
            documentation: item.documentation ? stringifyMarkdown(item.documentation) : null,
            insertText: typeof item.insertText === 'string' ? item.insertText : item.insertText?.value ?? null,
            sortText: item.sortText ?? null,
            extension: entry.extension.id,
          });
        }
      }
      return { items };
    });

    c.onRequest('provider/format', async (params = {}) => {
      const doc = this.docs.get(Uri.parse(params.uri).toString());
      if (!doc) return { edits: [] };
      for (const entry of this.providers.values()) {
        if (entry.kind !== 'formatting' || !matchesSelector(entry.selector, doc)) continue;
        const edits = await entry.provider.provideDocumentFormattingEdits(doc, params.options || {}, noCancel());
        if (edits?.length) {
          return { edits: edits.map((e) => ({ range: e.range, newText: e.newText })), extension: entry.extension.id };
        }
      }
      return { edits: [] };
    });

    c.onRequest('shutdown', async () => {
      for (const extension of this.extensions.values()) await this.deactivate(extension);
      return { ok: true };
    });

    c.onNotification('exit', () => process.exit(0));
  }
}

function noCancel() {
  return { isCancellationRequested: false, onCancellationRequested: () => new Disposable(() => {}) };
}

function stringifyMarkdown(value) {
  if (value == null) return '';
  if (typeof value === 'string') return value;
  if (typeof value.value === 'string') return value.value;
  return String(value);
}

const LANGUAGE_BY_EXT = {
  '.js': 'javascript', '.jsx': 'javascriptreact', '.ts': 'typescript', '.tsx': 'typescriptreact',
  '.json': 'json', '.md': 'markdown', '.rs': 'rust', '.py': 'python', '.go': 'go', '.c': 'c',
  '.h': 'c', '.cpp': 'cpp', '.hpp': 'cpp', '.cs': 'csharp', '.java': 'java', '.rb': 'ruby',
  '.php': 'php', '.sh': 'shellscript', '.html': 'html', '.css': 'css', '.scss': 'scss',
  '.toml': 'toml', '.yaml': 'yaml', '.yml': 'yaml', '.sql': 'sql', '.lua': 'lua',
};

function languageFor(file) {
  return LANGUAGE_BY_EXT[path.extname(file).toLowerCase()] || 'plaintext';
}

// ---------------------------------------------------------------------------
// CLI entry point
// ---------------------------------------------------------------------------

function parseArgs(argv) {
  const args = {};
  for (let i = 0; i < argv.length; i++) {
    if (!argv[i].startsWith('--')) continue;
    const key = argv[i].slice(2);
    const value = argv[i + 1] && !argv[i + 1].startsWith('--') ? argv[++i] : 'true';
    args[key] = value;
  }
  return args;
}

function main() {
  const args = parseArgs(process.argv.slice(2));
  const extensionsDir = args['extensions-dir'] || require('../paths').extensionsDir();

  // stdout is the RPC channel: anything an extension console.logs would corrupt
  // the stream, so route it to stderr.
  const stdout = process.stdout;
  console.log = (...a) => process.stderr.write(a.map(String).join(' ') + '\n');
  console.info = console.log;
  console.debug = console.log;

  const connection = new Connection(process.stdin, stdout);
  const host = new ExtensionHost({ connection, extensionsDir, storageDir: args['storage-dir'] });
  connection.on('close', () => process.exit(0));
  process.on('uncaughtException', (err) => {
    host.log('error', `uncaught exception: ${err?.stack || err}`);
  });
  process.on('unhandledRejection', (err) => {
    host.log('error', `unhandled rejection: ${err?.stack || err}`);
  });
  host.log('info', `extension host ready (api ${API_VERSION}, dir ${extensionsDir})`);
}

if (require.main === module) main();

module.exports = { ExtensionHost, TextDocument, matchesSelector, API_VERSION, languageFor, main };
