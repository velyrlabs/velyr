'use strict';
// util.parseArgs: tokenizes argv (long, short, grouped shorts, --k=v, `--`) and folds tokens into
// values/positionals. Strict mode throws ERR_PARSE_ARGS_* errors.
const errors = require('internal/errors');

const fail = (code, message, Base = TypeError) => errors.make(Base, code, message);

function parseArgs(config = {}) {
  errors.validateObject(config, 'config');
  const args = config.args ?? [];
  const strict = config.strict ?? true;
  const allowPositionals = config.allowPositionals ?? !strict;
  const returnTokens = config.tokens ?? false;
  const allowNegative = config.allowNegative ?? false;
  const options = config.options ?? {};
  for (const [name, option] of Object.entries(options)) {
    if (option === null || typeof option !== 'object') throw errors.invalidArgType(`options.${name}`, 'Object', option);
    if (option.type !== 'string' && option.type !== 'boolean') {
      throw errors.invalidArgValue(`options.${name}.type`, option.type, "must be one of: 'string', 'boolean'");
    }
    if (option.short !== undefined && (typeof option.short !== 'string' || option.short.length !== 1)) {
      throw errors.invalidArgValue(`options.${name}.short`, option.short, 'must be a single character');
    }
  }
  const isString = (name) => options[name]?.type === 'string';
  const shortToLong = (short) => Object.keys(options).find((k) => options[k].short === short) ?? short;

  const tokens = [];
  let rest = false;
  for (let i = 0; i < args.length; i++) {
    const arg = args[i];
    const index = i;
    if (rest) { tokens.push({ kind: 'positional', index, value: arg }); continue; }
    if (arg === '--') { tokens.push({ kind: 'option-terminator', index }); rest = true; continue; }
    if (arg.startsWith('--') ) {
      const eq = arg.indexOf('=');
      const name = eq === -1 ? arg.slice(2) : arg.slice(2, eq);
      if (eq !== -1) { tokens.push({ kind: 'option', name, rawName: `--${name}`, index, value: arg.slice(eq + 1), inlineValue: true }); continue; }
      if (isString(name) && i + 1 < args.length) {
        tokens.push({ kind: 'option', name, rawName: arg, index, value: args[i + 1], inlineValue: false });
        i++;
      } else {
        tokens.push({ kind: 'option', name, rawName: arg, index, value: undefined, inlineValue: undefined });
      }
      continue;
    }
    if (arg.length > 1 && arg[0] === '-') {
      for (let c = 1; c < arg.length; c++) {
        const short = arg[c];
        const name = shortToLong(short);
        if (isString(name)) {
          const inline = arg.slice(c + 1);
          if (inline !== '') {
            tokens.push({ kind: 'option', name, rawName: `-${short}`, index, value: inline, inlineValue: true });
          } else if (i + 1 < args.length) {
            tokens.push({ kind: 'option', name, rawName: `-${short}`, index, value: args[i + 1], inlineValue: false });
            i++;
          } else {
            tokens.push({ kind: 'option', name, rawName: `-${short}`, index, value: undefined, inlineValue: undefined });
          }
          break;
        }
        tokens.push({ kind: 'option', name, rawName: `-${short}`, index, value: undefined, inlineValue: undefined });
      }
      continue;
    }
    tokens.push({ kind: 'positional', index, value: arg });
  }

  const values = { __proto__: null };
  const positionals = [];
  for (const token of tokens) {
    if (token.kind === 'positional') {
      if (!allowPositionals) {
        throw fail('ERR_PARSE_ARGS_UNEXPECTED_POSITIONAL', `Unexpected argument '${token.value}'. This command does not take positional arguments`);
      }
      positionals.push(token.value);
      continue;
    }
    if (token.kind !== 'option') continue;
    let { name } = token;
    const option = options[name];
    if (strict) {
      if (option === undefined) {
        if (allowNegative && name.startsWith('no-') && options[name.slice(3)]?.type === 'boolean') {
          token.name = name.slice(3);
          token.value = false;
          values[token.name] = false;
          continue;
        }
        throw fail('ERR_PARSE_ARGS_UNKNOWN_OPTION', `Unknown option '${token.rawName}'`);
      }
      if (option.type === 'string' && typeof token.value !== 'string') {
        throw fail('ERR_PARSE_ARGS_INVALID_OPTION_VALUE', `Option '${token.rawName} <value>' argument missing`);
      }
      if (option.type === 'boolean' && token.value != null) {
        throw fail('ERR_PARSE_ARGS_INVALID_OPTION_VALUE', `Option '${token.rawName}' does not take an argument`);
      }
    }
    const value = option?.type === 'string' || typeof token.value === 'string' ? token.value : true;
    if (option?.multiple) (values[name] ??= []).push(value);
    else values[name] = value;
  }
  for (const [name, option] of Object.entries(options)) {
    if (option.default !== undefined && values[name] === undefined) values[name] = option.default;
  }
  const result = { values, positionals };
  if (returnTokens) result.tokens = tokens;
  return result;
}

module.exports = { parseArgs };
