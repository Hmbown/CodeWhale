// Execution facade passed into backends. Locally it is the plain exec module;
// remote agents get the same shape, so every backend works unchanged on either
// side of an ssh hop.
import * as exec from "./exec.mjs";
export { exec };
export default exec;
