'use strict';
// Minimal Express 4 look-alike, written for these tests (the real package is not downloaded).
// Kept from express: createApplication(), app.use/get/post/put/delete/all, app.listen, path params,
// req.query/params/path/get, res.status/set/get/send/json/end, next(err) and error middleware, and
// the way express grafts its request/response onto http.IncomingMessage / http.ServerResponse.
// Left out: app.set/engine/render, sub-apps, Router(), content negotiation, ETag, static files,
// cookies, body parsing, trust proxy, regexp or wildcard paths.
const http = require('http');
const { Buffer } = require('buffer');

const req = Object.create(http.IncomingMessage.prototype);
const res = Object.create(http.ServerResponse.prototype);

req.get = function get(name) {
  return this.headers[String(name).toLowerCase()];
};

res.status = function status(code) {
  this.statusCode = code;
  return this;
};
res.set = res.header = function set(name, value) {
  this.setHeader(name, value);
  return this;
};
res.get = function get(name) {
  return this.getHeader(name);
};
res.send = function send(body) {
  if (body !== null && typeof body === 'object' && !Buffer.isBuffer(body) && !(body instanceof Uint8Array)) return this.json(body);
  if (typeof body === 'string' && !this.get('Content-Type')) this.set('Content-Type', 'text/html; charset=utf-8');
  if (body === undefined || body === null) body = '';
  this.end(typeof body === 'number' ? String(body) : body);
  return this;
};
res.json = function json(value) {
  if (!this.get('Content-Type')) this.set('Content-Type', 'application/json; charset=utf-8');
  return this.send(JSON.stringify(value));
};

function compile(path) {
  const keys = [];
  const source = path
    .split('/')
    .map((part) => {
      if (part.startsWith(':')) {
        keys.push(part.slice(1));
        return '([^/]+)';
      }
      return part.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
    })
    .join('/');
  return { keys, regexp: new RegExp(`^${source}/?$`) };
}

function createApplication() {
  const stack = [];
  const add = (method, path, handlers, prefix) => {
    const layer = { method, handlers: handlers.flat(), prefix };
    if (path !== null) Object.assign(layer, compile(path), { path });
    stack.push(layer);
  };

  function app(request, response) {
    app.handle(request, response);
  }

  app.use = function use(...args) {
    const path = typeof args[0] === 'string' ? args.shift() : '/';
    add(null, path, args, true);
    return app;
  };
  for (const method of ['get', 'post', 'put', 'delete', 'patch']) {
    app[method] = (path, ...handlers) => {
      add(method.toUpperCase(), path, handlers, false);
      return app;
    };
  }
  app.all = (path, ...handlers) => {
    add('*', path, handlers, false);
    return app;
  };

  app.handle = function handle(request, response) {
    Object.setPrototypeOf(request, req);
    Object.setPrototypeOf(response, res);
    request.res = response;
    response.req = request;
    const url = new URL(request.url, 'http://placeholder');
    request.path = url.pathname;
    request.query = Object.fromEntries(url.searchParams);
    request.params = {};
    request.app = app;
    let index = 0;
    const next = (error) => {
      const layer = stack[index++];
      if (!layer) {
        if (error) {
          response.statusCode = error.status || 500;
          response.end(error.status ? String(error.message) : 'Internal Server Error');
        } else {
          response.statusCode = 404;
          response.end(`Cannot ${request.method} ${request.path}`);
        }
        return;
      }
      let match = null;
      if (layer.regexp) {
        if (layer.prefix) {
          const prefix = layer.path === '/' ? '' : layer.path.replace(/\/$/, '');
          if (!(request.path === prefix || request.path.startsWith(`${prefix}/`) || prefix === '')) return next(error);
          match = [];
        } else {
          if (layer.method !== '*' && layer.method !== request.method && !(layer.method === 'GET' && request.method === 'HEAD')) return next(error);
          match = layer.regexp.exec(request.path);
          if (!match) return next(error);
        }
      }
      layer.keys?.forEach((key, i) => {
        request.params[key] = decodeURIComponent(match[i + 1]);
      });
      let position = 0;
      const step = (failure) => {
        const handler = layer.handlers[position++];
        if (!handler) return next(failure);
        try {
          if (failure) {
            if (handler.length === 4) handler(failure, request, response, step);
            else step(failure);
          } else if (handler.length < 4) {
            handler(request, response, step);
          } else {
            step();
          }
        } catch (thrown) {
          step(thrown);
        }
      };
      step(error);
    };
    next();
  };

  app.listen = function listen(...args) {
    return http.createServer(app).listen(...args);
  };
  return app;
}

createApplication.json = () => (request, response, next) => {
  if (!/json/.test(request.headers['content-type'] || '')) return next();
  const parts = [];
  request.on('data', (part) => parts.push(part));
  request.on('end', () => {
    try {
      request.body = JSON.parse(Buffer.concat(parts).toString() || 'null');
      next();
    } catch (error) {
      error.status = 400;
      next(error);
    }
  });
};

module.exports = createApplication;
module.exports.application = createApplication;
module.exports.request = req;
module.exports.response = res;
