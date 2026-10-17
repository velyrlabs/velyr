let _initClass;
let _B;
class B {
  static {
    [_B, _initClass] = _applyDecs(this, [dec], []).c;
  }
  static {
    _initClass();
  }
}
export { _B as B };
