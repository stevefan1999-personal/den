// Navigator / NavigatorUAData. Host strings come from `natives.host`
// (OS, arch, crate version, uname release, hardwareConcurrency); this file
// is the JS-visible shape, matching txiki.js's navigator polyfill with the
// brand set to `den`.
(function (natives, api) {
  const {
    os,
    arch,
    version,
    hardwareConcurrency,
    bitness,
    kernelRelease,
  } = natives.host;

  const getNavigatorPlatform = (machine, platform) => {
    if (platform === "macos") return "MacIntel";
    if (platform === "windows") return "Win32";
    if (platform === "linux") {
      if (machine === "x86" || machine === "i686" || machine === "i386") {
        return "Linux i686";
      }
      if (machine === "x86_64") return "Linux x86_64";
      return `Linux ${machine}`;
    }
    if (platform === "freebsd") {
      if (machine === "i386") return "FreeBSD i386";
      if (machine === "amd64" || machine === "x86_64") return "FreeBSD amd64";
      return `FreeBSD ${machine}`;
    }
    if (platform === "openbsd") {
      if (machine === "i386") return "OpenBSD i386";
      if (machine === "amd64" || machine === "x86_64") return "OpenBSD amd64";
      return `OpenBSD ${machine}`;
    }
    return `${platform} ${machine}`;
  };

  const getUADataPlatform = (platform) => {
    switch (platform) {
      case "macos":
        return "macOS";
      case "windows":
        return "Windows";
      case "linux":
        return "Linux";
      case "freebsd":
        return "FreeBSD";
      case "openbsd":
        return "OpenBSD";
      default:
        return platform;
    }
  };

  const getArchitecture = (machine) => {
    switch (machine) {
      case "x86_64":
      case "amd64":
      case "x86":
      case "i686":
      case "i386":
        return "x86";
      case "arm64":
      case "aarch64":
      case "arm":
        return "arm";
      default:
        return machine;
    }
  };

  const getPlatformVersion = (release) => {
    const parts = String(release).split(".");
    return [parts[0] ?? "0", parts[1] ?? "0", parts[2] ?? "0"].join(".");
  };

  const majorVersion = String(version).split(".")[0];

  class NavigatorUAData {
    #brands;
    #mobile;
    #platform;

    constructor() {
      this.#brands = Object.freeze([
        Object.freeze({ brand: "den", version: majorVersion }),
      ]);
      this.#mobile = false;
      this.#platform = getUADataPlatform(os);
    }

    get brands() {
      return this.#brands;
    }

    get mobile() {
      return this.#mobile;
    }

    get platform() {
      return this.#platform;
    }

    getHighEntropyValues(hints) {
      if (!Array.isArray(hints)) {
        return Promise.reject(new TypeError("hints must be an array"));
      }
      const result = {
        brands: this.#brands,
        mobile: this.#mobile,
        platform: this.#platform,
      };
      for (const hint of hints) {
        switch (hint) {
          case "architecture":
            result.architecture = getArchitecture(arch);
            break;
          case "bitness":
            result.bitness = bitness;
            break;
          case "fullVersionList":
            result.fullVersionList = Object.freeze([
              Object.freeze({ brand: "den", version: String(version) }),
            ]);
            break;
          case "model":
            result.model = "";
            break;
          case "platformVersion":
            result.platformVersion = getPlatformVersion(kernelRelease);
            break;
          case "wow64":
            result.wow64 = false;
            break;
          case "formFactors":
            result.formFactors = Object.freeze(["Desktop"]);
            break;
        }
      }
      return Promise.resolve(result);
    }

    toJSON() {
      return {
        brands: this.#brands,
        mobile: this.#mobile,
        platform: this.#platform,
      };
    }

    get [Symbol.toStringTag]() {
      return "NavigatorUAData";
    }
  }

  const userAgentData = new NavigatorUAData();

  class Navigator {
    get userAgent() {
      return `den/${version}`;
    }

    get hardwareConcurrency() {
      return hardwareConcurrency;
    }

    get platform() {
      return getNavigatorPlatform(arch, os);
    }

    get userAgentData() {
      return userAgentData;
    }

    get [Symbol.toStringTag]() {
      return "Navigator";
    }
  }

  return {
    ...api,
    NavigatorUAData,
    navigator: new Navigator(),
  };
})
