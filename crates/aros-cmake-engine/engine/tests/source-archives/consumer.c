int archive_one(void);
int archive_two(void);
int main(void) { return archive_one() + archive_two() == 46 ? 0 : 1; }
