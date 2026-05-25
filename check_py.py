import importlib
for m in ['openpyxl','pandas','pdfplumber','selenium','playwright','requests','PIL','bs4','lxml','numpy','matplotlib','flask','fastapi']:
    try:
        importlib.import_module(m)
        print(f'{m}: OK')
    except:
        print(f'{m}: MISS')
