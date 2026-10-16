'use strict';
// Color tables and the column layout used by util.inspect for long arrays.
const colors = {
  reset: [0, 0], bold: [1, 22], dim: [2, 22], italic: [3, 23], underline: [4, 24], blink: [5, 25],
  inverse: [7, 27], hidden: [8, 28], strikethrough: [9, 29], doubleunderline: [21, 24],
  black: [30, 39], red: [31, 39], green: [32, 39], yellow: [33, 39], blue: [34, 39],
  magenta: [35, 39], cyan: [36, 39], white: [37, 39],
  bgBlack: [40, 49], bgRed: [41, 49], bgGreen: [42, 49], bgYellow: [43, 49], bgBlue: [44, 49],
  bgMagenta: [45, 49], bgCyan: [46, 49], bgWhite: [47, 49],
  framed: [51, 54], overlined: [53, 55],
  gray: [90, 39], redBright: [91, 39], greenBright: [92, 39], yellowBright: [93, 39],
  blueBright: [94, 39], magentaBright: [95, 39], cyanBright: [96, 39], whiteBright: [97, 39],
  bgGray: [100, 49], bgRedBright: [101, 49], bgGreenBright: [102, 49], bgYellowBright: [103, 49],
  bgBlueBright: [104, 49], bgMagentaBright: [105, 49], bgCyanBright: [106, 49], bgWhiteBright: [107, 49],
};
for (const [alias, name] of [['grey', 'gray'], ['blackBright', 'gray'], ['bgGrey', 'bgGray'], ['bgBlackBright', 'bgGray']]) {
  colors[alias] = colors[name];
}
const styles = {
  special: 'cyan', number: 'yellow', bigint: 'yellow', boolean: 'yellow', undefined: 'grey',
  null: 'bold', string: 'green', symbol: 'green', date: 'magenta', regexp: 'red', module: 'underline',
};

const ansiRe = /\u001b\[\d+m/g;
const widthOf = (text, colored) => (colored ? text.replace(ansiRe, '') : text).length;

// Lays out long arrays of short entries in aligned columns.
function groupArrayElements(ctx, output, value) {
  let total = 0;
  let maxLength = 0;
  let length = output.length;
  if (ctx.maxArrayLength < output.length) length--;
  const separator = 2;
  const widths = new Array(length);
  for (let i = 0; i < length; i++) {
    widths[i] = widthOf(output[i], ctx.colors);
    total += widths[i] + separator;
    if (maxLength < widths[i]) maxLength = widths[i];
  }
  const actualMax = maxLength + separator;
  if (actualMax * 3 + ctx.indentationLvl < ctx.breakLength && (total / actualMax > 5 || maxLength <= 6)) {
    const bias = Math.sqrt(actualMax - total / output.length);
    const biasedMax = Math.max(actualMax - 3 - bias, 1);
    const columns = Math.min(
      Math.round(Math.sqrt(2.5 * biasedMax * length) / biasedMax),
      Math.floor((ctx.breakLength - ctx.indentationLvl) / actualMax),
      ctx.compact * 4,
      15,
    );
    if (columns <= 1) return output;
    const grouped = [];
    const lineWidths = [];
    for (let i = 0; i < columns; i++) {
      let widest = 0;
      for (let j = i; j < output.length; j += columns) if (widths[j] > widest) widest = widths[j];
      lineWidths.push(widest + separator);
    }
    let padStart = true;
    if (value !== undefined) {
      for (let i = 0; i < output.length; i++) {
        if (typeof value[i] !== 'number' && typeof value[i] !== 'bigint') { padStart = false; break; }
      }
    }
    for (let i = 0; i < length; i += columns) {
      const max = Math.min(i + columns, length);
      let line = '';
      let j = i;
      for (; j < max - 1; j++) {
        const pad = lineWidths[j - i] + output[j].length - widths[j];
        line += padStart ? `${output[j]}, `.padStart(pad, ' ') : `${output[j]}, `.padEnd(pad, ' ');
      }
      if (padStart) line += output[j].padStart(lineWidths[j - i] + output[j].length - widths[j] - separator, ' ');
      else line += output[j];
      grouped.push(line);
    }
    if (ctx.maxArrayLength < output.length) grouped.push(output[length]);
    output = grouped;
  }
  return output;
}

module.exports = { colors, styles, groupArrayElements, widthOf };
