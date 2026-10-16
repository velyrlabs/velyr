'use strict';
// node:http. Written for this runtime; see client.js, server.js and response.js for how it maps
// onto io-http and what it leaves out.
const { Agent, ClientRequest, get, request } = require('internal/http/client');
const { METHODS, OutgoingMessage, STATUS_CODES, validateHeaderName, validateHeaderValue } = require('internal/http/common');
const { IncomingMessage } = require('internal/http/incoming');
const { Server } = require('internal/http/server');
const { ServerResponse } = require('internal/http/response');

function createServer(options, listener) {
  return new Server(options, listener);
}

module.exports = {
  Agent,
  ClientRequest,
  IncomingMessage,
  METHODS,
  OutgoingMessage,
  STATUS_CODES,
  Server,
  ServerResponse,
  createServer,
  get,
  maxHeaderSize: 16384,
  request,
  setMaxIdleHTTPParsers() {},
  validateHeaderName,
  validateHeaderValue,
};
// `http.globalAgent = agent` is how programs swap the default agent.
Object.defineProperty(module.exports, 'globalAgent', {
  get: () => Agent.globalAgent,
  set(agent) {
    Agent.globalAgent = agent;
  },
  enumerable: true,
  configurable: true,
});
