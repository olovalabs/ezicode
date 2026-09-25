'use strict';
// A normal VS Code extension — nothing ezicode-specific in here.
// The exact same file runs in VS Code, Theia and ezicode.

const vscode = require('vscode');

function activate(context) {
  const status = vscode.window.createStatusBarItem(vscode.StatusBarAlignment.Right, 100);
  status.text = '$(rocket) hello';
  status.tooltip = 'Hello ezicode is alive';
  status.command = 'hello.sayHello';
  status.show();

  context.subscriptions.push(
    vscode.commands.registerCommand('hello.sayHello', async () => {
      const greeting = vscode.workspace
        .getConfiguration('hello')
        .get('greeting', 'Hello from a VS Code extension running inside ezicode');
      const choice = await vscode.window.showInformationMessage(greeting, 'Nice', 'Docs');
      if (choice === 'Docs') {
        await vscode.env.openExternal(vscode.Uri.parse('https://github.com/olovalabs/ezicode'));
      }
      return greeting;
    })
  );

  context.subscriptions.push(
    vscode.commands.registerCommand('hello.insertBanner', async () => {
      const editor = vscode.window.activeTextEditor;
      if (!editor) {
        vscode.window.showWarningMessage('Open a file first');
        return false;
      }
      await editor.edit((builder) => {
        builder.insert(editor.selection.start, `// built with ezicode — ${new Date().toISOString()}\n`);
      });
      return true;
    })
  );

  context.subscriptions.push(
    vscode.commands.registerCommand('hello.countWords', () => {
      const editor = vscode.window.activeTextEditor;
      if (!editor) return 0;
      const words = editor.document.getText().split(/\s+/).filter(Boolean).length;
      vscode.window.setStatusBarMessage(`${words} words`, 3000);
      return words;
    })
  );

  context.subscriptions.push(
    vscode.languages.registerHoverProvider(['javascript', 'typescript'], {
      provideHover(document, position) {
        const range = document.getWordRangeAtPosition(position);
        if (!range) return null;
        const word = document.getText(range);
        const md = new vscode.MarkdownString();
        md.appendMarkdown(`**${word}**\n\n`);
        md.appendMarkdown(`from \`hello-ezicode\` — ${document.lineCount} lines in this file`);
        return new vscode.Hover(md, range);
      },
    })
  );

  context.subscriptions.push(
    vscode.languages.registerCompletionItemProvider(
      ['javascript', 'typescript'],
      {
        provideCompletionItems() {
          const item = new vscode.CompletionItem('ezicode', vscode.CompletionItemKind.Snippet);
          item.detail = 'contributed by hello-ezicode';
          item.insertText = 'ezicode';
          return [item];
        },
      },
      '.'
    )
  );

  // Anything returned here is visible to other extensions via
  // vscode.extensions.getExtension('ezicode.hello-ezicode').exports
  return { sayHelloTo: (name) => `hello ${name}` };
}

function deactivate() {}

module.exports = { activate, deactivate };
