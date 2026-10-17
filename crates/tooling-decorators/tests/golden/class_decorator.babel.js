let _initClass;
let _A;
class A {
  static {
    [_A, _initClass] = _applyDecs(this, [dec], []).c;
  }
  static {
    _initClass();
  }
}
