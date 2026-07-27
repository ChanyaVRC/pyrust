def update_provider(provider, name, value):
    provider.update({name: value})


def pop_provider(provider, name):
    return provider.pop(name)
