import _ from 'lodash';

const eq = (a, b) => { if (JSON.stringify(a) !== JSON.stringify(b)) throw new Error(`${JSON.stringify(a)} !== ${JSON.stringify(b)}`); };
eq(_.chunk([1, 2, 3, 4, 5], 2), [[1, 2], [3, 4], [5]]);
eq(_.get({ a: { b: [{ c: 3 }] } }, 'a.b[0].c'), 3);
eq(_.merge({ a: 1, b: { c: 2 } }, { b: { d: 3 } }), { a: 1, b: { c: 2, d: 3 } });
eq(_.groupBy([1.1, 1.9, 2.3], Math.floor), { 1: [1.1, 1.9], 2: [2.3] });
eq(_.camelCase('hello world-foo'), 'helloWorldFoo');
eq(_.uniq([1, 1, 2]), [1, 2]);
eq(_.template('hi <%= n %>')({ n: 'x' }), 'hi x');
const debounced = _.debounce(() => {}, 1);
debounced.cancel();
console.log('ok');
