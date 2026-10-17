let _initProto, _call_p;
class C {
  static {
    [_call_p, _initProto] = _applyDecs(this, [], [[dec, 2, "p", function () {}]], 0, _ => #p in _).e;
  }
  constructor() {
    _initProto(this);
  }
  #p = _call_p;
}
