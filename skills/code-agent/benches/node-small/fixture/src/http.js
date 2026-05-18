export function statusLabel(status) {
  if (status >= 200 && status <= 299) {
    return "ok";
  }
  if (status >= 400 && status <= 499) {
    return "client_error";
  }
  if (status >= 500 && status <= 599) {
    return "server_error";
  }
  return "unknown";
}
