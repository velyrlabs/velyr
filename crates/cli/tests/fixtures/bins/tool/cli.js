#!/usr/bin/env node
'use strict';
const args = process.argv.slice(2);
if (args.includes('--help')) {
  console.log('usage: tool [options]');
} else {
  console.log('args:' + JSON.stringify(args));
}
