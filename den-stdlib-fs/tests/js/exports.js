import { assertEquals } from "den:assert";
import * as ns from "den:fs";

assertEquals(
  Object.keys(ns).sort().join(","),
  "canonicalize,copy,createDir,createDirAll,hardLink,metadata,read,readDir,readLink," +
    "readToString,removeDir,removeDirAll,removeFile,rename,setPermissions,symlinkMetadata,write",
);
