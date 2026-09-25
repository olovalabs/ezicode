'use strict';
// End-to-end proof: a real, unmodified VS Code extension (examples/hello-ezicode)
// loaded by the host, driven over the same JSON-RPC channel the Rust editor uses.

const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { spawn } = require('node:child_process');

const { Connection } = require('../src/host/rpc');

const HOST = path.join(__dirname, '..', 'src', 'host', 'host.js');
const EXAMPLE = path.join(__dirname, '..', 'examples', 'hello-ezicode');

function startHost() {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'ezicode-host-'));
  const extensionsDir = path.join(root, 'extensions');
  fs.cpSync(EXAMPLE, path.join(extensionsDir, 'ezicode.hello-ezicode'), { recursive: true });

  const child = spawn(process.execPath, [HOST, '--extensions-dir', extensionsDir], {
    stdio: ['pipe', 'pipe', 'pipe'],
  });
  const stderr = [];
  child.stderr.on('data', (c) => stderr.push(c.toString()));

  const connection = new Connection(child.stdout, child.stdin);
  const notifications = [];
  for (const method of [
    'log',
    'commands/registered',
    'window/statusBarItem',
    'window/setStatusBarMessage',
    'extension/activated',
    'providers/changed',
  ]) {
    connection.onNotification(method, (params) => notifications.push({ method, params }));
  }

  return { child, connection, notifications, extensionsDir, stderr };
}

test('the host loads, activates and drives a VS Code extension', async (t) => {
  const { child, connection, notifications, stderr } = startHost();
  t.after(() => child.kill());

  // --- initialize ---------------------------------------------------------
  const init = await connection.sendRequest('initialize', {
    workspaceFolders: [process.cwd()],
    settings: { 'hello.greeting': 'greetings from the test' },
  });

  assert.equal(init.apiVersion, '1.85.0');
  assert.equal(init.extensions.length, 1);
  assert.equal(init.extensions[0].id, 'ezicode.hello-ezicode');
  assert.equal(init.extensions[0].lazy, true);
  assert.deepEqual(init.extensions[0].unsupportedActivation, []);
  assert.deepEqual(
    init.commands.map((c) => c.command).sort(),
    ['hello.countWords', 'hello.insertBanner', 'hello.sayHello']
  );
  assert.equal(init.commands[0].category, 'Hello');

  // --- activation ---------------------------------------------------------
  const activated = await connection.sendRequest('activate', { event: 'onStartupFinished' });
  assert.deepEqual(activated.activated, ['ezicode.hello-ezicode']);

  const commands = await connection.sendRequest('commands/list', {});
  assert.deepEqual(commands.registered.sort(), ['hello.countWords', 'hello.insertBanner', 'hello.sayHello']);

  // The extension created a status bar item during activate().
  const statusBar = notifications.find((n) => n.method === 'window/statusBarItem');
  assert.ok(statusBar, 'expected a status bar item notification');
  assert.equal(statusBar.params.text, '$(rocket) hello');
  assert.equal(statusBar.params.command, 'hello.sayHello');

  // --- commands + messages ------------------------------------------------
  const messages = [];
  connection.onRequest('window/showMessage', (params) => {
    messages.push(params);
    return 'Nice';
  });

  const hello = await connection.sendRequest('commands/execute', { command: 'hello.sayHello' });
  assert.equal(hello.result, 'greetings from the test'); // read via workspace.getConfiguration
  assert.equal(messages.length, 1);
  assert.equal(messages[0].kind, 'info');
  assert.equal(messages[0].message, 'greetings from the test');
  assert.deepEqual(messages[0].items, ['Nice', 'Docs']);

  // --- documents, hover and completion ------------------------------------
  const uri = 'file:///tmp/demo.js';
  connection.sendNotification('document/didOpen', {
    uri,
    languageId: 'javascript',
    version: 1,
    text: 'const answer = 42;\nconsole.log(answer);\n',
  });
  connection.sendNotification('editor/didChangeActive', {
    uri,
    selection: { startLine: 1, startCharacter: 0, endLine: 1, endCharacter: 0 },
  });

  const hover = await connection.sendRequest('provider/hover', {
    uri,
    position: { line: 0, character: 8 },
  });
  assert.equal(hover.extension, 'ezicode.hello-ezicode');
  assert.match(hover.contents[0], /\*\*answer\*\*/);
  assert.match(hover.contents[0], /3 lines in this file/);

  const completion = await connection.sendRequest('provider/completion', {
    uri,
    position: { line: 1, character: 7 },
    triggerCharacter: '.',
  });
  assert.deepEqual(completion.items.map((i) => i.label), ['ezicode']);
  assert.equal(completion.items[0].detail, 'contributed by hello-ezicode');

  // Commands can read the active editor's document.
  const words = await connection.sendRequest('commands/execute', { command: 'hello.countWords' });
  assert.equal(words.result, 5);

  // --- edits round-trip through the editor --------------------------------
  const applied = [];
  connection.onRequest('workspace/applyEdit', (params) => {
    applied.push(params);
    return { applied: true };
  });
  const banner = await connection.sendRequest('commands/execute', { command: 'hello.insertBanner' });
  assert.equal(banner.result, true);
  assert.equal(applied.length, 1);
  assert.match(applied[0].edits[0].edits[0].newText, /built with ezicode/);
  assert.equal(applied[0].edits[0].uri, uri);

  // --- errors surface as JSON-RPC errors, they do not kill the host -------
  await assert.rejects(
    connection.sendRequest('commands/execute', { command: 'does.not.exist' }),
    /command not found: does.not.exist/
  );

  const after = await connection.sendRequest('extensions/list', {});
  assert.equal(after.extensions[0].active, true);

  await connection.sendRequest('shutdown', {});
  assert.ok(!stderr.join('').includes('uncaught exception'));
});

test('lazy activation: onCommand and onLanguage load extensions on demand', async (t) => {
  const { child, connection, extensionsDir } = startHost();
  t.after(() => child.kill());

  // A second extension that only wakes up for markdown files or its command —
  // the activation pattern almost every real extension uses.
  const dir = path.join(extensionsDir, 'acme.lazy');
  fs.mkdirSync(dir, { recursive: true });
  fs.writeFileSync(
    path.join(dir, 'package.json'),
    JSON.stringify({
      name: 'lazy',
      publisher: 'acme',
      version: '1.0.0',
      main: './main.js',
      activationEvents: ['onCommand:lazy.run', 'onLanguage:markdown'],
      contributes: { commands: [{ command: 'lazy.run', title: 'Run' }] },
    })
  );
  fs.writeFileSync(
    path.join(dir, 'main.js'),
    `const vscode = require('vscode');
     exports.activate = (context) => {
       context.subscriptions.push(vscode.commands.registerCommand('lazy.run', () => 'ran'));
       context.subscriptions.push(
         vscode.languages.registerDocumentFormattingEditProvider('markdown', {
           provideDocumentFormattingEdits(document) {
             const last = document.lineAt(document.lineCount - 1);
             return [vscode.TextEdit.insert(last.range.end, '\\n<!-- formatted by acme.lazy -->')];
           },
         })
       );
     };`
  );

  await connection.sendRequest('initialize', { workspaceFolders: [], settings: {} });

  // Nothing is active yet, and startup activation must not wake it.
  await connection.sendRequest('activate', { event: 'onStartupFinished' });
  let list = await connection.sendRequest('extensions/list', {});
  assert.equal(list.extensions.find((e) => e.id === 'acme.lazy').active, false);

  // Executing the command activates the extension first.
  const ran = await connection.sendRequest('commands/execute', { command: 'lazy.run' });
  assert.equal(ran.result, 'ran');
  list = await connection.sendRequest('extensions/list', {});
  assert.equal(list.extensions.find((e) => e.id === 'acme.lazy').active, true);

  // Its formatter is now available for markdown documents.
  const uri = 'file:///tmp/readme.md';
  connection.sendNotification('document/didOpen', {
    uri,
    languageId: 'markdown',
    version: 1,
    text: '# title\n',
  });
  const formatted = await connection.sendRequest('provider/format', { uri, options: { tabSize: 2 } });
  assert.equal(formatted.extension, 'acme.lazy');
  assert.match(formatted.edits[0].newText, /formatted by acme.lazy/);
});

test('a disabled extension is not activated', async (t) => {
  const { child, connection, extensionsDir } = startHost();
  t.after(() => child.kill());

  fs.writeFileSync(
    path.join(extensionsDir, 'ezicode.hello-ezicode', '.ezicode.json'),
    JSON.stringify({ id: 'ezicode.hello-ezicode', enabled: false })
  );

  const init = await connection.sendRequest('initialize', { workspaceFolders: [], settings: {} });
  assert.equal(init.extensions[0].enabled, false);

  const activated = await connection.sendRequest('activate', { event: 'onStartupFinished' });
  assert.deepEqual(activated.activated, []);
});
