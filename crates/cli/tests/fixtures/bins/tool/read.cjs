'use strict';
// Prints the file named by argv[2]; needs FsRead, so it shows what the caller allowed.
const fs = require('node:fs');
console.log('read:' + fs.readFileSync(process.argv[2], 'utf8').trim());
