export function displayName(user) {
  const initials = `${user.firstName[0]}${user.lastName[0]}`.toUpperCase();
  return `${user.firstName} ${user.lastName} (${initials})`;
}
