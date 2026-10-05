#if !defined(SDK_STALE_DECOY)
#error decoy object must be compiled with its distinct stale Make flag
#endif

int sdk_stale_decoy_symbol(void)
{
    return 101;
}
