let _initProto, _init_x, _init_extra_x, _init_y, _init_extra_y;
class A {
	static {
		[_init_y, _init_extra_y, _init_x, _init_extra_x, _initProto] = _applyDecs2311(this, [], [
			[
				dec,
				2,
				"m"
			],
			[
				dec,
				8,
				"x"
			],
			[
				dec,
				1,
				"y"
			],
			[
				dec,
				3,
				"g"
			]
		]).e;
	}
	m() {}
	static x = _init_x(1);
	#y_accessor_storage = (_initProto(this), _init_y(this, 2));
	get y() {
		return this.#y_accessor_storage;
	}
	set y(v) {
		this.#y_accessor_storage = v;
	}
	get g() {
		return 1;
	}
	#initFields = (_init_extra_y(this), void 0);
	static {
		_init_extra_x();
	}
}
