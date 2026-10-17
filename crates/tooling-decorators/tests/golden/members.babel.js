let _initProto, _init_x, _init_extra_x, _init_y, _init_extra_y;
class A {
  static {
    [_init_y, _init_extra_y, _init_x, _init_extra_x, _initProto] = _applyDecs(this, [], [[dec, 2, "m"], [dec, 8, "x"], [dec, 1, "y"], [dec, 3, "g"]]).e;
  }
  constructor() {
    _init_extra_y(this);
  }
  m() {}
  static x = _init_x(1);
  #A = (_initProto(this), _init_y(this, 2));
  get y() {
    return this.#A;
  }
  set y(v) {
    this.#A = v;
  }
  get g() {
    return 1;
  }
  static {
    _init_extra_x();
  }
}
