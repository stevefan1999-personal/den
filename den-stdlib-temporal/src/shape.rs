//! Stamp Temporal constructors, statics, and prototype methods with test262
//! `name` / `length` / property descriptors, and RequireInternalSlot branding.
//!
//! rquickjs installs methods as non-configurable own properties, so this
//! rebuilds each interface as a `class` (writable:false `prototype`) and
//! rehomes values that the Rust class still stamps onto the original proto.

/// WebIDL-shaped Temporal namespace plus per-interface method metadata.
pub const DEFINE_INTERFACE_SHAPE: &str = r#"
(namespace, now, interfaces, global) => {
  const CONSTRUCTOR_LENGTH = {
    Instant: 1,
    Duration: 0,
    PlainDate: 3,
    PlainTime: 0,
    PlainDateTime: 3,
    PlainYearMonth: 2,
    PlainMonthDay: 2,
    ZonedDateTime: 2,
  };
  const STATIC_LENGTH = {
    from: 1,
    compare: 2,
    fromEpochNanoseconds: 1,
    fromEpochMilliseconds: 1,
  };
  const PROTO_LENGTH = {
    abs: 0,
    add: 1,
    equals: 1,
    getTimeZoneTransition: 1,
    negated: 0,
    round: 1,
    since: 1,
    startOfDay: 0,
    subtract: 1,
    toInstant: 0,
    toJSON: 0,
    toLocaleString: 0,
    toPlainDate: 0,
    toPlainDateTime: 0,
    toPlainMonthDay: 0,
    toPlainTime: 0,
    toPlainYearMonth: 0,
    toString: 0,
    toZonedDateTime: 1,
    toZonedDateTimeISO: 1,
    total: 1,
    until: 1,
    valueOf: 0,
    with: 1,
    withCalendar: 1,
    withPlainTime: 0,
    withTimeZone: 1,
  };
  const PROTO_LENGTH_OVERRIDE = {
    "PlainYearMonth.toPlainDate": 1,
    "PlainMonthDay.toPlainDate": 1,
  };
  const RENAME = {
    toJson: "toJSON",
    to_json: "toJSON",
    toZonedDateTimeIso: "toZonedDateTimeISO",
    to_string: "toString",
    value_of: "valueOf",
  };
  const REQUIRED_METHODS = {
    Instant: [
      "add", "equals", "round", "since", "subtract", "toJSON", "toLocaleString",
      "toString", "toZonedDateTimeISO", "until", "valueOf",
    ],
    Duration: [
      "abs", "add", "negated", "round", "subtract", "toJSON", "toLocaleString",
      "toString", "total", "valueOf", "with",
    ],
    PlainDate: [
      "add", "equals", "since", "subtract", "toJSON", "toLocaleString",
      "toPlainDateTime", "toPlainMonthDay", "toPlainYearMonth", "toString",
      "toZonedDateTime", "until", "valueOf", "with", "withCalendar",
    ],
    PlainTime: [
      "add", "equals", "round", "since", "subtract", "toJSON", "toLocaleString",
      "toString", "until", "valueOf", "with",
    ],
    PlainDateTime: [
      "add", "equals", "round", "since", "subtract", "toJSON", "toLocaleString",
      "toPlainDate", "toPlainTime", "toString", "toZonedDateTime", "until",
      "valueOf", "with", "withCalendar", "withPlainTime",
    ],
    PlainYearMonth: [
      "add", "equals", "since", "subtract", "toJSON", "toLocaleString",
      "toPlainDate", "toString", "until", "valueOf", "with",
    ],
    PlainMonthDay: [
      "equals", "toJSON", "toLocaleString", "toPlainDate", "toString", "valueOf",
      "with",
    ],
    ZonedDateTime: [
      "add", "equals", "getTimeZoneTransition", "round", "since", "startOfDay",
      "subtract", "toInstant", "toJSON", "toLocaleString", "toPlainDate",
      "toPlainDateTime", "toPlainTime", "toString", "until", "valueOf", "with",
      "withCalendar", "withPlainTime", "withTimeZone",
    ],
  };
  const REQUIRED_GETTERS = {
    Instant: ["epochNanoseconds", "epochMilliseconds"],
    Duration: [
      "years", "months", "weeks", "days", "hours", "minutes", "seconds",
      "milliseconds", "microseconds", "nanoseconds", "sign", "blank",
    ],
    PlainDate: [
      "calendarId", "era", "eraYear", "year", "month", "monthCode", "day",
      "dayOfWeek", "dayOfYear", "weekOfYear", "yearOfWeek", "daysInWeek",
      "daysInMonth", "daysInYear", "monthsInYear", "inLeapYear",
    ],
    PlainTime: [
      "hour", "minute", "second", "millisecond", "microsecond", "nanosecond",
    ],
    PlainDateTime: [
      "calendarId", "era", "eraYear", "year", "month", "monthCode", "day",
      "hour", "minute", "second", "millisecond", "microsecond", "nanosecond",
      "dayOfWeek", "dayOfYear", "weekOfYear", "yearOfWeek", "daysInWeek",
      "daysInMonth", "daysInYear", "monthsInYear", "inLeapYear",
    ],
    PlainYearMonth: [
      "calendarId", "era", "eraYear", "year", "month", "monthCode", "daysInYear",
      "daysInMonth", "monthsInYear", "inLeapYear",
    ],
    PlainMonthDay: ["calendarId", "monthCode", "day"],
    ZonedDateTime: [
      "calendarId", "timeZoneId", "era", "eraYear", "year", "month", "monthCode",
      "day", "hour", "minute", "second", "millisecond", "microsecond",
      "nanosecond", "epochNanoseconds", "epochMilliseconds", "dayOfWeek",
      "dayOfYear", "weekOfYear", "yearOfWeek", "daysInWeek", "daysInMonth",
      "daysInYear", "monthsInYear", "inLeapYear", "offset", "offsetNanoseconds",
      "hoursInDay",
    ],
  };
  const REQUIRED_STATICS = {
    Instant: ["from", "compare", "fromEpochNanoseconds", "fromEpochMilliseconds"],
    Duration: ["from", "compare"],
    PlainDate: ["from", "compare"],
    PlainTime: ["from", "compare"],
    PlainDateTime: ["from", "compare"],
    PlainYearMonth: ["from", "compare"],
    PlainMonthDay: ["from"],
    ZonedDateTime: ["from", "compare"],
  };
  const WITH_FIELDS = {
    Duration: [
      "years", "months", "weeks", "days", "hours", "minutes", "seconds",
      "milliseconds", "microseconds", "nanoseconds",
    ],
    PlainDate: ["year", "month", "monthCode", "day"],
    PlainTime: ["hour", "minute", "second", "millisecond", "microsecond", "nanosecond"],
    PlainDateTime: [
      "year", "month", "monthCode", "day", "hour", "minute", "second",
      "millisecond", "microsecond", "nanosecond",
    ],
    PlainYearMonth: ["year", "month", "monthCode"],
    PlainMonthDay: ["year", "month", "monthCode", "day"],
    ZonedDateTime: [
      "year", "month", "monthCode", "day", "hour", "minute", "second",
      "millisecond", "microsecond", "nanosecond", "offset",
    ],
  };

  const brands = [];

  const stamp = (fn, name, length) => {
    Object.defineProperty(fn, "name", {
      value: name,
      writable: false,
      enumerable: false,
      configurable: true,
    });
    Object.defineProperty(fn, "length", {
      value: length,
      writable: false,
      enumerable: false,
      configurable: true,
    });
    return fn;
  };

  const makeFn = (name, length, apply) => {
    const fn = {
      [name](...args) {
        return apply(this, args);
      },
    }[name];
    return stamp(fn, name, length);
  };

  const rehome = (value) => {
    if (value === null || typeof value !== "object") {
      return value;
    }
    const current = Object.getPrototypeOf(value);
    for (let i = 0; i < brands.length; i++) {
      const brand = brands[i];
      if (current === brand.original.prototype) {
        Object.setPrototypeOf(value, brand.proto);
        break;
      }
    }
    return value;
  };

  const isBrand = (value, wrapped, original) => {
    if (value === null || typeof value !== "object") {
      return false;
    }
    return value instanceof wrapped || Object.getPrototypeOf(value) === original.prototype;
  };

  const protoLength = (typeName, methodName) => {
    const override = PROTO_LENGTH_OVERRIDE[`${typeName}.${methodName}`];
    if (override !== undefined) {
      return override;
    }
    return PROTO_LENGTH[methodName] ?? 0;
  };

  const isObject = (value) =>
    value !== null && (typeof value === "object" || typeof value === "function");

  const toJsString = (value) => {
    if (typeof value === "symbol") {
      throw new TypeError("cannot convert a Symbol to a String");
    }
    return String(value);
  };

  const toIntegerWithTruncation = (value) => {
    if (typeof value === "bigint" || typeof value === "symbol") {
      throw new TypeError("cannot convert BigInt or Symbol to a Number");
    }
    const number = Number(value);
    if (!Number.isFinite(number)) {
      throw new RangeError("integer is not finite");
    }
    return Math.trunc(number);
  };

  const toIntegerIfIntegral = (value) => {
    const integer = toIntegerWithTruncation(value);
    if (Number(value) !== integer) {
      throw new RangeError("expected an integer");
    }
    return integer;
  };

  const toTemporalOverflow = (options) => {
    if (options === undefined) {
      return "constrain";
    }
    if (!isObject(options)) {
      throw new TypeError("options must be an object");
    }
    const value = options.overflow;
    if (value === undefined) {
      return "constrain";
    }
    const name = toJsString(value);
    if (name !== "constrain" && name !== "reject") {
      throw new RangeError("invalid overflow option");
    }
    return name;
  };

  const parseMonthCode = (code) => {
    const match = /^M(\d{2})$/.exec(toJsString(code));
    if (!match) {
      throw new RangeError("invalid monthCode");
    }
    return Number(match[1]);
  };

  const monthFromPartial = (partial, fallbackMonth) => {
    if (partial.month !== undefined && partial.monthCode !== undefined) {
      const fromCode = parseMonthCode(partial.monthCode);
      if (fromCode !== partial.month) {
        throw new RangeError("month and monthCode must agree");
      }
      return partial.month;
    }
    if (partial.month !== undefined) {
      return partial.month;
    }
    if (partial.monthCode !== undefined) {
      return parseMonthCode(partial.monthCode);
    }
    return fallbackMonth;
  };

  const constructWith = (typeName, self, partial, wrapped) => {
    switch (typeName) {
      case "Duration":
        return rehome(new wrapped(
          partial.years ?? self.years,
          partial.months ?? self.months,
          partial.weeks ?? self.weeks,
          partial.days ?? self.days,
          partial.hours ?? self.hours,
          partial.minutes ?? self.minutes,
          partial.seconds ?? self.seconds,
          partial.milliseconds ?? self.milliseconds,
          partial.microseconds ?? self.microseconds,
          partial.nanoseconds ?? self.nanoseconds,
        ));
      case "PlainDate":
        return rehome(new wrapped(
          partial.year ?? self.year,
          monthFromPartial(partial, self.month),
          partial.day ?? self.day,
          self.calendarId,
        ));
      case "PlainTime":
        return rehome(new wrapped(
          partial.hour ?? self.hour,
          partial.minute ?? self.minute,
          partial.second ?? self.second,
          partial.millisecond ?? self.millisecond,
          partial.microsecond ?? self.microsecond,
          partial.nanosecond ?? self.nanosecond,
        ));
      case "PlainDateTime":
        return rehome(new wrapped(
          partial.year ?? self.year,
          monthFromPartial(partial, self.month),
          partial.day ?? self.day,
          partial.hour ?? self.hour,
          partial.minute ?? self.minute,
          partial.second ?? self.second,
          partial.millisecond ?? self.millisecond,
          partial.microsecond ?? self.microsecond,
          partial.nanosecond ?? self.nanosecond,
          self.calendarId,
        ));
      case "PlainYearMonth":
        return rehome(new wrapped(
          partial.year ?? self.year,
          monthFromPartial(partial, self.month),
          self.calendarId,
        ));
      case "PlainMonthDay":
        return rehome(new wrapped(
          monthFromPartial(partial, parseMonthCode(self.monthCode)),
          partial.day ?? self.day,
          self.calendarId,
        ));
      case "ZonedDateTime": {
        const bag = {
          year: partial.year ?? self.year,
          month: monthFromPartial(partial, self.month),
          day: partial.day ?? self.day,
          hour: partial.hour ?? self.hour,
          minute: partial.minute ?? self.minute,
          second: partial.second ?? self.second,
          millisecond: partial.millisecond ?? self.millisecond,
          microsecond: partial.microsecond ?? self.microsecond,
          nanosecond: partial.nanosecond ?? self.nanosecond,
          timeZone: self.timeZoneId,
          calendar: self.calendarId,
          offset: partial.offset ?? self.offset,
        };
        return rehome(wrapped.from(bag));
      }
      default:
        throw new TypeError(`${typeName}.with is not implemented`);
    }
  };

  const makeFrom = (_typeName, originalFrom, original) =>
    makeFn("from", 1, (_self, args) => rehome(originalFrom.call(original, args[0], args[1])));

  const installIsoWith = (target, typeName, wrapped, original) => {
    if (ownFunction(original.prototype, "with")) {
      return;
    }
    const fieldKeys = WITH_FIELDS[typeName];
    if (!fieldKeys) {
      return;
    }
    const fn = makeFn("with", 1, (self, args) => {
      if (!isBrand(self, wrapped, original)) {
        throw new TypeError("with called on incompatible receiver");
      }
      const fields = args[0];
      const options = args[1];
      if (!isObject(fields)) {
        throw new TypeError("argument must be an object");
      }
      if (typeName !== "Duration") {
        if (fields.calendar !== undefined || fields.timeZone !== undefined) {
          throw new TypeError("with() does not accept calendar or timeZone");
        }
      }
      const partial = {};
      let present = false;
      for (const key of fieldKeys) {
        const value = fields[key];
        if (value === undefined) {
          continue;
        }
        present = true;
        if (key === "monthCode" || key === "offset") {
          partial[key] = toJsString(value);
        } else if (typeName === "Duration") {
          partial[key] = toIntegerIfIntegral(value);
        } else {
          partial[key] = toIntegerWithTruncation(value);
        }
      }
      if (!present) {
        throw new TypeError("invalid with() argument");
      }
      const result = constructWith(typeName, self, partial, wrapped);
      if (typeName !== "Duration") {
        toTemporalOverflow(options);
      }
      return result;
    });
    Object.defineProperty(target, "with", {
      value: fn,
      writable: true,
      enumerable: false,
      configurable: true,
    });
  };

  const installToPlainDate = (target, typeName, wrapped, original) => {
    if (typeName !== "PlainYearMonth" && typeName !== "PlainMonthDay") {
      return;
    }
    if (
      ownFunction(original.prototype, "toPlainDate") ||
      ownFunction(original.prototype, "to_plain_date")
    ) {
      return;
    }
    const fn = makeFn("toPlainDate", 1, (self, args) => {
      if (!isBrand(self, wrapped, original)) {
        throw new TypeError("toPlainDate called on incompatible receiver");
      }
      const item = args[0];
      if (!isObject(item)) {
        throw new TypeError("argument must be an object");
      }
      const PlainDate = specs.find((spec) => spec.name === "PlainDate")?.wrapped;
      if (typeName === "PlainYearMonth") {
        const day = item.day;
        if (day === undefined) {
          throw new TypeError("day is required");
        }
        return rehome(new PlainDate(self.year, self.month, toIntegerWithTruncation(day), self.calendarId));
      }
      const year = item.year;
      if (year === undefined) {
        throw new TypeError("year is required");
      }
      return rehome(
        new PlainDate(
          toIntegerWithTruncation(year),
          parseMonthCode(self.monthCode),
          self.day,
          self.calendarId,
        ),
      );
    });
    Object.defineProperty(target, "toPlainDate", {
      value: fn,
      writable: true,
      enumerable: false,
      configurable: true,
    });
  };

  const installIsoRound = (target, typeName, wrapped, original) => {
    if (
      typeName !== "PlainDateTime" &&
      typeName !== "PlainTime" &&
      typeName !== "ZonedDateTime"
    ) {
      return;
    }
    if (ownFunction(original.prototype, "round")) {
      return;
    }
    const fn = makeFn("round", 1, (self, args) => {
      if (!isBrand(self, wrapped, original)) {
        throw new TypeError("round called on incompatible receiver");
      }
      requireOptionsBag("round", args);
      const Instant = specs.find((spec) => spec.name === "Instant")?.wrapped;
      const ZonedDateTime = specs.find((spec) => spec.name === "ZonedDateTime")?.wrapped;
      const PlainTime = specs.find((spec) => spec.name === "PlainTime")?.wrapped;
      const options = args[0];
      if (typeName === "ZonedDateTime") {
        const rounded = Instant.fromEpochNanoseconds(self.epochNanoseconds).round(options);
        return rehome(rounded.toZonedDateTimeISO(self.timeZoneId));
      }
      if (typeName === "PlainDateTime") {
        const zoned = self.toZonedDateTime("UTC");
        const rounded = Instant.fromEpochNanoseconds(zoned.epochNanoseconds).round(options);
        return rehome(rounded.toZonedDateTimeISO("UTC").toPlainDateTime());
      }
      const zoned = ZonedDateTime.from({
        year: 1970,
        month: 1,
        day: 1,
        hour: self.hour,
        minute: self.minute,
        second: self.second,
        millisecond: self.millisecond,
        microsecond: self.microsecond,
        nanosecond: self.nanosecond,
        timeZone: "UTC",
      });
      const rounded = Instant.fromEpochNanoseconds(zoned.epochNanoseconds).round(options);
      const out = rounded.toZonedDateTimeISO("UTC");
      return rehome(
        new PlainTime(
          out.hour,
          out.minute,
          out.second,
          out.millisecond,
          out.microsecond,
          out.nanosecond,
        ),
      );
    });
    Object.defineProperty(target, "round", {
      value: fn,
      writable: true,
      enumerable: false,
      configurable: true,
    });
  };

  const ownFunction = (object, key) => {
    const desc = Object.getOwnPropertyDescriptor(object, key);
    if (desc && typeof desc.value === "function") {
      return desc.value;
    }
    return undefined;
  };

  const ownGetter = (object, key) => {
    const desc = Object.getOwnPropertyDescriptor(object, key);
    return desc && desc.get ? desc.get : undefined;
  };

  const requireOptionsBag = (specName, args) => {
    if (specName !== "round" && specName !== "total") {
      return;
    }
    if (args.length === 0 || args[0] === undefined) {
      throw new TypeError("options are required");
    }
    const options = args[0];
    if (
      options === null ||
      (typeof options !== "object" &&
        typeof options !== "function" &&
        typeof options !== "string")
    ) {
      throw new TypeError("options must be an object or string");
    }
  };

  const installMethod = (target, specName, originalFn, length, wrapped, original) => {
    const ignoreArgs = specName === "toLocaleString";
    const fn = makeFn(specName, length, (self, args) => {
      if (!isBrand(self, wrapped, original)) {
        throw new TypeError(`${specName} called on incompatible receiver`);
      }
      requireOptionsBag(specName, args);
      return rehome(originalFn.apply(self, ignoreArgs ? [] : args));
    });
    Object.defineProperty(target, specName, {
      value: fn,
      writable: true,
      enumerable: false,
      configurable: true,
    });
  };

  const installGetter = (target, specName, originalGet, wrapped, original) => {
    const getterName = `get ${specName}`;
    const getter = makeFn(getterName, 0, (self) => {
      if (!isBrand(self, wrapped, original)) {
        throw new TypeError(`${getterName} called on incompatible receiver`);
      }
      return rehome(originalGet.call(self));
    });
    Object.defineProperty(target, specName, {
      get: getter,
      enumerable: false,
      configurable: true,
    });
  };

  const installStubMethod = (target, specName, length, wrapped, original) => {
    if (Object.getOwnPropertyDescriptor(target, specName)) {
      return;
    }
    const fn = makeFn(specName, length, (self) => {
      if (!isBrand(self, wrapped, original)) {
        throw new TypeError(`${specName} called on incompatible receiver`);
      }
      throw new TypeError(`${specName} is not implemented`);
    });
    Object.defineProperty(target, specName, {
      value: fn,
      writable: true,
      enumerable: false,
      configurable: true,
    });
  };

  const installStubGetter = (target, specName, wrapped, original) => {
    if (Object.getOwnPropertyDescriptor(target, specName)) {
      return;
    }
    const getterName = `get ${specName}`;
    const getter = makeFn(getterName, 0, (self) => {
      if (!isBrand(self, wrapped, original)) {
        throw new TypeError(`${getterName} called on incompatible receiver`);
      }
      if (specName === "era" || specName === "eraYear") {
        return undefined;
      }
      throw new TypeError(`${specName} is not implemented`);
    });
    Object.defineProperty(target, specName, {
      get: getter,
      enumerable: false,
      configurable: true,
    });
  };

  const hide = (target, name) =>
    Object.defineProperty(target, name, {
      value: target[name],
      writable: true,
      enumerable: false,
      configurable: true,
    });

  const tag = (target, value) =>
    target &&
    Object.defineProperty(target, Symbol.toStringTag, {
      value,
      writable: false,
      enumerable: false,
      configurable: true,
    });

  const specs = interfaces.map((name) => {
    const original = namespace[name];
    const wrapped = class {
      constructor(...args) {
        const instance = Reflect.construct(original, args, new.target);
        if (Object.getPrototypeOf(instance) === original.prototype && new.target !== original) {
          const desired = new.target.prototype;
          if (desired !== null && typeof desired === "object") {
            Object.setPrototypeOf(instance, desired);
          }
        }
        return instance;
      }
    };
    stamp(wrapped, name, CONSTRUCTOR_LENGTH[name] ?? 0);
    return { name, original, wrapped, proto: wrapped.prototype };
  });
  brands.push(...specs);

  for (const spec of specs) {
    const { name, original, wrapped, proto } = spec;
    const originalProto = original.prototype;

    for (const key of Object.getOwnPropertyNames(original)) {
      if (key === "prototype" || key === "name" || key === "length") {
        continue;
      }
      const desc = Object.getOwnPropertyDescriptor(original, key);
      if (!desc || typeof desc.value !== "function") {
        continue;
      }
      const specName = RENAME[key] ?? key;
      const length = STATIC_LENGTH[specName] ?? desc.value.length;
      const fn = specName === "from"
        ? makeFrom(name, desc.value, original)
        : makeFn(specName, length, (_self, args) => rehome(desc.value.apply(original, args)));
      Object.defineProperty(wrapped, specName, {
        value: fn,
        writable: true,
        enumerable: false,
        configurable: true,
      });
    }
    for (const specName of REQUIRED_STATICS[name] ?? []) {
      if (!Object.getOwnPropertyDescriptor(wrapped, specName)) {
        const found = ownFunction(original, specName) ?? ownFunction(original, specName[0].toLowerCase() + specName.slice(1));
        if (found) {
          const length = STATIC_LENGTH[specName] ?? found.length;
          const fn = specName === "from"
            ? makeFrom(name, found, original)
            : makeFn(specName, length, (_self, args) => rehome(found.apply(original, args)));
          Object.defineProperty(wrapped, specName, {
            value: fn,
            writable: true,
            enumerable: false,
            configurable: true,
          });
        }
      }
    }

    for (const key of Object.getOwnPropertyNames(originalProto)) {
      if (key === "constructor") {
        continue;
      }
      const desc = Object.getOwnPropertyDescriptor(originalProto, key);
      if (!desc) {
        continue;
      }
      const specName = RENAME[key] ?? key;
      if (desc.get) {
        installGetter(proto, specName, desc.get, wrapped, original);
      } else if (typeof desc.value === "function") {
        installMethod(proto, specName, desc.value, protoLength(name, specName), wrapped, original);
      }
    }

    const rustToString =
      ownFunction(originalProto, "toString") ?? ownFunction(originalProto, "to_string");
    if (rustToString && rustToString !== Object.prototype.toString) {
      installMethod(proto, "toString", rustToString, 0, wrapped, original);
    }
    const rustValueOf =
      ownFunction(originalProto, "valueOf") ?? ownFunction(originalProto, "value_of");
    if (rustValueOf && rustValueOf !== Object.prototype.valueOf) {
      installMethod(proto, "valueOf", rustValueOf, 0, wrapped, original);
    }

    const toStringSource =
      ownFunction(proto, "toString") && ownFunction(proto, "toString") !== Object.prototype.toString
        ? ownFunction(originalProto, "toString") ?? ownFunction(originalProto, "to_string") ?? rustToString
        : rustToString;
    if (toStringSource && toStringSource !== Object.prototype.toString) {
      installMethod(proto, "toLocaleString", toStringSource, 0, wrapped, original);
    }

    for (const specName of REQUIRED_METHODS[name] ?? []) {
      installStubMethod(proto, specName, protoLength(name, specName), wrapped, original);
    }
    installIsoWith(proto, name, wrapped, original);
    installToPlainDate(proto, name, wrapped, original);
    installIsoRound(proto, name, wrapped, original);
    for (const specName of REQUIRED_GETTERS[name] ?? []) {
      const getter = ownGetter(originalProto, specName);
      if (getter && !Object.getOwnPropertyDescriptor(proto, specName)) {
        installGetter(proto, specName, getter, wrapped, original);
      } else {
        installStubGetter(proto, specName, wrapped, original);
      }
    }

    tag(proto, `Temporal.${name}`);
    namespace[name] = wrapped;
  }

  for (const name of Object.getOwnPropertyNames(now)) {
    const fn = now[name];
    if (typeof fn !== "function") {
      continue;
    }
    Object.defineProperty(now, name, {
      value: makeFn(name, 0, (self, args) => rehome(fn.apply(self, args))),
      writable: true,
      enumerable: false,
      configurable: true,
    });
  }
  tag(now, "Temporal.Now");
  namespace.Now = now;

  for (const name of Object.keys(namespace)) {
    hide(namespace, name);
  }
  tag(namespace, "Temporal");
  Object.defineProperty(global, "Temporal", {
    value: namespace,
    writable: true,
    enumerable: false,
    configurable: true,
  });
}
"#;
