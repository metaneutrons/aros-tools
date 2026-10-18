#ifndef ORDER
#error "ordered contract lost final definition"
#endif
#if ORDER != 1
#error "ordered contract changed final value"
#endif
#ifndef LANGUAGE_C
#error "C contract missing"
#endif
#ifdef LANGUAGE_CXX
#error "CXX contract leaked into C"
#endif
_Static_assert(sizeof(VALUE) == 4, "quoted whitespace changed");
_Static_assert(sizeof(APOSTROPHE) == 4, "quoted apostrophe changed");
int ordered_contract_c(void) { return ORDER; }
