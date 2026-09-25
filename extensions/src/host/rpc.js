'use strict';
// JSON-RPC 2.0 with LSP-style `Content-Length` framing.
//
// Deliberately the same framing ezicode's LSP client already speaks
// (`app/src/lsp/client.rs`), so the Rust side can reuse its reader/writer for
// the extension host instead of growing a second transport.

const { EventEmitter } = require('node:events');

class Connection extends EventEmitter {
  /**
   * @param {NodeJS.ReadableStream} input
   * @param {NodeJS.WritableStream} output
   */
  constructor(input, output) {
    super();
    this.input = input;
    this.output = output;
    this.buffer = Buffer.alloc(0);
    this.nextId = 1;
    this.pending = new Map();
    this.requestHandlers = new Map();
    this.notificationHandlers = new Map();

    input.on('data', (chunk) => this._onData(chunk));
    input.on('end', () => this.emit('close'));
  }

  onRequest(method, handler) {
    this.requestHandlers.set(method, handler);
    return this;
  }

  onNotification(method, handler) {
    this.notificationHandlers.set(method, handler);
    return this;
  }

  sendNotification(method, params) {
    this._write({ jsonrpc: '2.0', method, params });
  }

  sendRequest(method, params) {
    const id = this.nextId++;
    return new Promise((resolve, reject) => {
      this.pending.set(id, { resolve, reject });
      this._write({ jsonrpc: '2.0', id, method, params });
    });
  }

  _write(message) {
    const body = Buffer.from(JSON.stringify(message), 'utf8');
    this.output.write(`Content-Length: ${body.length}\r\n\r\n`);
    this.output.write(body);
  }

  _onData(chunk) {
    this.buffer = Buffer.concat([this.buffer, chunk]);
    for (;;) {
      const headerEnd = this.buffer.indexOf('\r\n\r\n');
      if (headerEnd === -1) return;
      const header = this.buffer.toString('ascii', 0, headerEnd);
      const match = /content-length:\s*(\d+)/i.exec(header);
      if (!match) {
        // Unrecoverable desync — drop the bad header and resynchronise.
        this.buffer = this.buffer.subarray(headerEnd + 4);
        continue;
      }
      const length = Number(match[1]);
      const start = headerEnd + 4;
      if (this.buffer.length < start + length) return;
      const body = this.buffer.toString('utf8', start, start + length);
      this.buffer = this.buffer.subarray(start + length);
      let message;
      try {
        message = JSON.parse(body);
      } catch (err) {
        this.emit('error', err);
        continue;
      }
      this._dispatch(message);
    }
  }

  async _dispatch(message) {
    if (message.id !== undefined && message.method === undefined) {
      const pending = this.pending.get(message.id);
      if (!pending) return;
      this.pending.delete(message.id);
      if (message.error) pending.reject(Object.assign(new Error(message.error.message), message.error));
      else pending.resolve(message.result);
      return;
    }

    if (message.id === undefined) {
      const handler = this.notificationHandlers.get(message.method);
      if (handler) {
        try {
          await handler(message.params);
        } catch (err) {
          this.emit('error', err);
        }
      }
      return;
    }

    const handler = this.requestHandlers.get(message.method);
    if (!handler) {
      this._write({
        jsonrpc: '2.0',
        id: message.id,
        error: { code: -32601, message: `method not found: ${message.method}` },
      });
      return;
    }
    try {
      const result = await handler(message.params);
      this._write({ jsonrpc: '2.0', id: message.id, result: result ?? null });
    } catch (err) {
      this._write({
        jsonrpc: '2.0',
        id: message.id,
        error: { code: -32000, message: String(err && err.message ? err.message : err), data: err?.stack },
      });
    }
  }
}

module.exports = { Connection };
