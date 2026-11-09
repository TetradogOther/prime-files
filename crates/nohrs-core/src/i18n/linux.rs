//! The texts that are not the same on Linux, for each language of the
//! interface: the Portuguese key, then the English, the Spanish and the
//! Catalan.
//!
//! The strings of [`super::strings`] are written for the Windows program:
//! they name the Explorer's menus, the PowerShell, the Windows' own folder
//! handler and the Listary. On Linux the desktop's programs have other
//! names, and the settings page says those. The table is read before the
//! main one, so a key found here is shown as it is written here.
//!
//! Only the keys that really are different are here; everything else comes
//! from the main table.

pub(super) static LINUX: &[(&str, &str, &str, &str)] = &[
    (
        "Integração com o Windows",
        "Integration with the desktop",
        "Integración con el escritorio",
        "Integració amb l'escriptori",
    ),
    (
        "Abrir no Prime Files as pastas do Windows e de outros programas.",
        "Open the folders of the desktop and of other programs in Prime Files.",
        "Abrir en Prime Files las carpetas del escritorio y de otros programas.",
        "Obre al Prime Files les carpetes de l'escriptori i d'altres programes.",
    ),
    (
        "Pastas do Windows",
        "System folders",
        "Carpetas del sistema",
        "Carpetes del sistema",
    ),
    (
        "Explorador padrão do Windows",
        "Default file manager",
        "Explorador de archivos predeterminado",
        "Gestor de fitxers per defecte",
    ),
    (
        "Pastas, drives e Win+E abrem no Prime Files.",
        "The folders, the disks and the file manager open in Prime Files.",
        "Las carpetas, las unidades y el explorador de archivos se abren en Prime Files.",
        "Les carpetes, les unitats i el gestor de fitxers s'obren al Prime Files.",
    ),
    (
        "Pastas, drives, Win+E e o ícone do Explorador na barra de tarefas abrem no Prime Files. Só neste usuário do Windows; desligar devolve tudo ao Explorador.",
        "The folders, the disks and the file manager open in Prime Files. Only for this user; turning it off gives everything back to the file manager of the desktop.",
        "Las carpetas, las unidades y el explorador de archivos se abren en Prime Files. Solo para este usuario; desactivarlo devuelve todo al explorador de archivos del escritorio.",
        "Les carpetes, les unitats i el gestor de fitxers s'obren al Prime Files. Només per a aquest usuari; desactivar-ho torna tot al gestor de fitxers de l'escriptori.",
    ),
    (
        "Pastas, unidades, Win+E e o ícone do Explorador na barra de tarefas passam a abrir aqui.",
        "The folders, the disks and the file manager open here from now on.",
        "Las carpetas, las unidades y el explorador de archivos se abren aquí a partir de ahora.",
        "Les carpetes, les unitats i el gestor de fitxers s'obriran aquí a partir d'ara.",
    ),
    (
        "Pastas, Win+E e o ícone do Explorador na barra de tarefas abrem no Prime Files.",
        "The folders and the file manager open in Prime Files.",
        "Las carpetas y el explorador de archivos se abren en Prime Files.",
        "Les carpetes i el gestor de fitxers s'obren al Prime Files.",
    ),
    (
        "Pastas voltaram a abrir no Explorador do Windows.",
        "The folders open in the file manager of the desktop again.",
        "Las carpetas vuelven a abrirse en el explorador de archivos del escritorio.",
        "Les carpetes tornen a obrir-se al gestor de fitxers de l'escriptori.",
    ),
    (
        "Iniciar com o Windows",
        "Start with the session",
        "Iniciar con la sesión",
        "Inicia amb la sessió",
    ),
    (
        "Fica pronto, minimizado, desde a entrada no Windows.",
        "Ready and minimized, from the moment the session starts.",
        "Listo y minimizado, desde que empieza la sesión.",
        "A punt i minimitzat, des que comença la sessió.",
    ),
    (
        "Abre minimizado quando você entra no Windows, para que \"Mostrar na pasta\" de qualquer programa caia aqui na hora.",
        "Opens minimized when the session starts, so that \"Show in folder\" from any program lands here at once.",
        "Se abre minimizado al iniciar la sesión, para que \"Mostrar en la carpeta\" de cualquier programa llegue aquí al instante.",
        "S'obre minimitzat en iniciar la sessió, perquè \"Mostra a la carpeta\" de qualsevol programa arribi aquí a l'instant.",
    ),
    (
        "Abrir a pasta no Explorador do Windows",
        "Open the folder in the file manager",
        "Abrir la carpeta en el explorador de archivos",
        "Obre la carpeta al gestor de fitxers",
    ),
    (
        "Abrir a pasta no PowerShell",
        "Open the folder in the Terminal",
        "Abrir la carpeta en la Terminal",
        "Obre la carpeta al Terminal",
    ),
    (
        "Abrir a pasta no Prompt de Comando",
        "Open the folder in a terminal",
        "Abrir la carpeta en una terminal",
        "Obre la carpeta en una terminal",
    ),
    (
        "Prompt de Comando",
        "Terminal",
        "Terminal",
        "Terminal",
    ),
    (
        "Explorador do Windows",
        "File manager",
        "Explorador de archivos",
        "Gestor de fitxers",
    ),
    (
        "Menu clássico do Windows",
        "Classic menu",
        "Menú clásico",
        "Menú clàssic",
    ),
    (
        "O botão direito abre direto o menu antigo do Windows, com Renomear, Copiar e Colar escritos, sem passar por \"Mostrar mais opções\".",
        "Right-click opens the classic menu straight away, with Rename, Copy and Paste spelled out.",
        "El botón derecho abre directamente el menú clásico, con Cambiar nombre, Copiar y Pegar escritos.",
        "El botó dret obre directament el menú clàssic, amb Canviar nom, Copiar i Enganxar escrits.",
    ),
    (
        "Com o menu clássico ligado, o botão direito abre o menu antigo do Windows no lugar deste.",
        "With the classic menu on, right-click opens the old menu instead of this one.",
        "Con el menú clásico activado, el clic derecho abre el menú antiguo en lugar de este.",
        "Amb el menú clàssic activat, el clic dret obre el menú antic en lloc d'aquest.",
    ),
    (
        "Itens do Windows",
        "System items",
        "Elementos del sistema",
        "Elements del sistema",
    ),
    (
        "Itens do Windows escondidos",
        "Hidden system items",
        "Elementos del sistema ocultos",
        "Elements del sistema amagats",
    ),
    (
        "\"Itens do Windows\" são os do Windows e dos programas instalados (Abrir com, Enviar para, 7-Zip…). Desligue para um menu curto, só com o que está aqui.",
        "\"System items\" are the ones of the desktop and of the installed programs (Open with, Send to…). Turn them off for a short menu, with only what is here.",
        "\"Elementos del sistema\" son los del escritorio y de los programas instalados (Abrir con, Enviar a…). Desactívalos para un menú corto, solo con lo que está aquí.",
        "\"Elements del sistema\" són els de l'escriptori i dels programes instal·lats (Obre amb, Envia a…). Desactiva'ls per a un menú curt, només amb el que hi ha aquí.",
    ),
    (
        "Clique com o botão direito num item do Windows, no menu, para escondê-lo. Clique aqui para trazê-lo de volta.",
        "Right-click a system item in the menu to hide it. Click here to bring it back.",
        "Haz clic derecho en un elemento del sistema, en el menú, para ocultarlo. Haz clic aquí para recuperarlo.",
        "Fes clic dret en un element del sistema, al menú, per amagar-lo. Fes clic aquí per recuperar-lo.",
    ),
    (
        "Nenhum. Clique com o botão direito num item do Windows, no menu, para escondê-lo.",
        "None. Right-click a system item in the menu to hide it.",
        "Ninguno. Haz clic derecho en un elemento del sistema, en el menú, para ocultarlo.",
        "Cap. Fes clic dret en un element del sistema, al menú, per amagar-lo.",
    ),
    (
        "Windows não consegue reproduzir este arquivo",
        "The system cannot play this file",
        "El sistema no puede reproducir este archivo",
        "El sistema no pot reproduir aquest fitxer",
    ),
    (
        "O Windows não abre este formato; o FFmpeg faz uma cópia leve para a prévia.",
        "The system does not open this format; FFmpeg makes a light copy for the preview.",
        "El sistema no abre este formato; FFmpeg hace una copia ligera para la vista previa.",
        "El sistema no obre aquest format; l'FFmpeg en fa una còpia lleugera per a la previsualització.",
    ),
    (
        "O Windows não abre este formato de vídeo. Instale o FFmpeg para ver a prévia.",
        "The system does not open this video format. Install FFmpeg to see the preview.",
        "El sistema no abre este formato de vídeo. Instale FFmpeg para ver la vista previa.",
        "El sistema no obre aquest format de vídeo. Instal·leu l'FFmpeg per veure la previsualització.",
    ),
    (
        "Um explorador de arquivos rápido para o Windows. Vamos deixá-lo do seu jeito em menos de um minuto.",
        "A fast file manager for the desktop. Let's make it yours in under a minute.",
        "Un explorador de archivos rápido para el escritorio. Vamos a dejarlo a tu gusto en menos de un minuto.",
        "Un gestor de fitxers ràpid per a l'escriptori. Deixem-lo al teu gust en menys d'un minut.",
    ),
    (
        "Um explorador de arquivos rápido para o Windows. Vamos deixá-lo do seu jeito em menos de um minuto: cada escolha vale na hora, e a janela de exemplo mostra como fica.",
        "A fast file manager for the desktop. Let's make it yours in under a minute: every choice applies at once, and the example window shows how it looks.",
        "Un explorador de archivos rápido para el escritorio. Vamos a dejarlo a tu gusto en menos de un minuto: cada elección se aplica al instante, y la ventana de ejemplo muestra cómo queda.",
        "Un gestor de fitxers ràpid per a l'escriptori. Deixem-lo al teu gust en menys d'un minut: cada elecció s'aplica a l'instant, i la finestra d'exemple mostra com queda.",
    ),
    (
        "Igual ao Windows",
        "Like the desktop",
        "Igual que el escritorio",
        "Igual que l'escriptori",
    ),
    (
        "Igual ao Windows troca sozinho quando o Windows troca.",
        "Match the desktop switches by itself when the desktop does.",
        "Igual que el escritorio cambia solo cuando cambia el escritorio.",
        "Igual que l'escriptori canvia sol quan canvia l'escriptori.",
    ),
    (
        "Automático segue o formato regional do Windows (Hora e idioma › Região); neste computador, {language}.",
        "Automatic follows the regional format of the desktop; on this computer, {language}.",
        "Automático sigue el formato regional del escritorio; en este equipo, {language}.",
        "Automàtic segueix el format regional de l'escriptori; en aquest equip, {language}.",
    ),
    (
        "Automático segue o formato regional do Windows; neste computador, {language}.",
        "Automatic follows the regional format of the desktop; on this computer, {language}.",
        "Automático sigue el formato regional del escritorio; en este equipo, {language}.",
        "Automàtic segueix el format regional de l'escriptori; en aquest equip, {language}.",
    ),
    (
        "Escolha se as pastas do Windows abrem aqui. Dá para desfazer a qualquer momento, e nada muda para os outros usuários do computador.",
        "Choose whether the folders of the desktop open here. It can be undone at any time, and nothing changes for the other users of the computer.",
        "Elige si las carpetas del escritorio se abren aquí. Se puede deshacer en cualquier momento, y nada cambia para los demás usuarios del equipo.",
        "Esculliu si les carpetes de l'escriptori s'obren aquí. Es pot desfer en qualsevol moment, i no canvia res per a la resta d'usuaris de l'equip.",
    ),
    (
        "Onde abre uma pasta mandada pela Área de Trabalho, pelo Win+E ou por outro programa.",
        "Where a folder sent by the desktop, by the file manager or by another program opens.",
        "Dónde se abre una carpeta enviada por el escritorio, por el explorador de archivos o por otro programa.",
        "On s'obre una carpeta enviada per l'escriptori, pel gestor de fitxers o per un altre programa.",
    ),
    (
        "Abre a busca do computador por cima de qualquer programa, como o Listary. Numa janela de abrir ou salvar arquivo, o que você escolher vai para ela. Ctrl+K também abre, dentro do Prime Files.",
        "Opens the search over any program, like the Listary did. In a window to open or save a file, what you choose goes to it. Ctrl+K opens it too, inside Prime Files.",
        "Abre la búsqueda por encima de cualquier programa, como hacía el Listary. En una ventana de abrir o guardar un archivo, lo que elijas va a ella. Ctrl+K también la abre, dentro de Prime Files.",
        "Obre la cerca per damunt de qualsevol programa, com feia el Listary. En una finestra per obrir o desar un fitxer, allò que tries hi va. També l'obre amb Ctrl+K, dins el Prime Files.",
    ),
    (
        "Busca rápida, o Listary do Prime Files",
        "Quick search, the Listary of Prime Files",
        "Búsqueda rápida, el Listary de Prime Files",
        "Cerca ràpida, el Listary del Prime Files",
    ),
    (
        "Procura uma versão nova ao abrir, baixa em segundo plano e instala quando você fecha o Prime Files.",
        "Looks for a new version when it opens and downloads it in the background; the package manager installs it.",
        "Busca una versión nueva al abrir y la descarga en segundo plano; el gestor de paquetes la instala.",
        "Busca una versió nova en obrir i la baixa en segon pla; el gestor de paquets la instal·la.",
    ),
];
